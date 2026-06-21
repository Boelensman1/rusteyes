use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::macos_helper::MacOSHelperBackend;
use crate::scheduler::{BreakSchedule, BreakScheduler, ScheduledBreak};
use crate::sync_protocol::SyncEvent;
use crate::sync_transport::{SyncTransport, SyncTransportError, SyncTransportEvent};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
use std::time::Duration;
use tracing::{trace, warn};

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config {
        breaks, lock, sync, ..
    } = Config::load()?;
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync)?;
    let backend = X11ActivityBackend::spawn(lock)?;

    run_with_backend(schedule, backend, &sync_transport);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config {
        breaks, lock, sync, ..
    } = Config::load()?;
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync)?;
    let backend = MacOSHelperBackend::spawn(lock)?;

    run_with_backend(schedule, backend, &sync_transport);
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn run() -> Result<(), crate::Error> {
    Err(crate::Error::unsupported_platform())
}

fn run_with_backend(
    schedule: BreakSchedule,
    backend: BackendActor,
    sync_transport: &SyncTransport,
) {
    let sync_runtime = RuntimeSync::new(sync_transport.event_receiver(), sync_transport);
    run_with_event_sources(schedule, backend, sync_runtime);
}

fn run_with_event_sources(
    schedule: BreakSchedule,
    backend: BackendActor,
    sync_runtime: RuntimeSync<'_>,
) {
    let backend_event_receiver = backend.event_receiver().clone_receiver();
    let scheduler = BreakScheduler::new(schedule);
    let mut daemon = DaemonRuntime {
        scheduler,
        backend,
        backend_event_receiver,
        sync_event_receiver: sync_runtime.event_receiver,
        sync_broadcaster: sync_runtime.broadcaster,
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

struct DaemonRuntime<'a> {
    scheduler: BreakScheduler,
    backend: BackendActor,
    backend_event_receiver: flume::Receiver<RuntimeEvent>,
    sync_event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
    sync_broadcaster: &'a dyn SyncEventBroadcaster,
    disable_mode: DisableMode,
    current_break: Option<CurrentBreakState>,
}

impl DaemonRuntime<'_> {
    fn run(&mut self) {
        while let Some(input) = self.next_input() {
            if !self.handle_input(input) {
                break;
            }
        }
    }

    fn next_input(&mut self) -> Option<RuntimeInput> {
        loop {
            let selected = match &self.sync_event_receiver {
                Some(sync_event_receiver) => flume::Selector::new()
                    .recv(&self.backend_event_receiver, SelectedRuntimeInput::Backend)
                    .recv(sync_event_receiver, SelectedRuntimeInput::SyncTransport)
                    .wait(),
                None => flume::Selector::new()
                    .recv(&self.backend_event_receiver, SelectedRuntimeInput::Backend)
                    .wait(),
            };

            match selected {
                SelectedRuntimeInput::Backend(Ok(event)) => {
                    return Some(RuntimeInput::Backend(event));
                }
                SelectedRuntimeInput::Backend(Err(_)) => {
                    warn!("backend event channel closed");
                    return None;
                }
                SelectedRuntimeInput::SyncTransport(Ok(event)) => {
                    return Some(RuntimeInput::SyncTransport(event));
                }
                SelectedRuntimeInput::SyncTransport(Err(_)) => {
                    warn!("sync transport event channel closed");
                    self.sync_event_receiver = None;
                }
            }
        }
    }

    fn handle_input(&mut self, input: RuntimeInput) -> bool {
        match input {
            RuntimeInput::Backend(event) => self.handle_event(event),
            RuntimeInput::SyncTransport(event) => self.handle_sync_transport_event(event),
        }
    }

    fn handle_event(&mut self, event: RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::ActiveTimeElapsed(elapsed) => {
                self.broadcast_active_time(elapsed);
                return self.advance_active(elapsed);
            }
            RuntimeEvent::WallClockElapsed(elapsed) => self.advance_wall_clock(elapsed),
            RuntimeEvent::BreakFinished => return self.finish_break(),
            RuntimeEvent::LockAfterCurrentBreak => self.request_lock_after_current_break(),
            RuntimeEvent::StartManualBreak(name) => return self.start_manual_break(&name),
            RuntimeEvent::Disable(DisableRequest::For(duration)) => {
                return self.disable_for(duration);
            }
            RuntimeEvent::Disable(DisableRequest::UntilRestart) => {
                return self.disable_until_restart();
            }
            RuntimeEvent::Enable => self.enable(),
            RuntimeEvent::Shutdown => return false,
        }

        true
    }

    fn handle_sync_transport_event(&mut self, event: SyncTransportEvent) -> bool {
        match event {
            SyncTransportEvent::Domain {
                peer_id,
                event: SyncEvent::ActiveTimeElapsed { elapsed },
            } => {
                trace!(peer_id = %peer_id, ?elapsed, "applying synced active time");
                self.advance_active(elapsed)
            }
            event => {
                trace!(?event, "received sync transport event");
                true
            }
        }
    }

    fn broadcast_active_time(&self, elapsed: Duration) {
        match self
            .sync_broadcaster
            .broadcast_sync_event(SyncEvent::ActiveTimeElapsed { elapsed })
        {
            Ok(peer_count) => {
                trace!(?elapsed, peer_count, "broadcast synced active time");
            }
            Err(error) => {
                warn!(%error, "failed to broadcast synced active time");
            }
        }
    }

    fn advance_active(&mut self, elapsed: Duration) -> bool {
        if let Some(scheduled_break) = self.scheduler.advance_active(elapsed) {
            self.start_break(scheduled_break)
        } else {
            true
        }
    }

    fn start_manual_break(&mut self, name: &str) -> bool {
        if let Some(scheduled_break) = self.scheduler.start_manual_break(name) {
            self.start_break(scheduled_break)
        } else {
            true
        }
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) -> bool {
        self.current_break = Some(CurrentBreakState::for_break(&scheduled_break));
        self.handle_command(BackendCommand::StartBreak(scheduled_break))
    }

    fn finish_break(&mut self) -> bool {
        let should_lock = self
            .current_break
            .take()
            .is_some_and(CurrentBreakState::lock_after);

        if self.scheduler.finish_break() {
            self.handle_command(BackendCommand::FinishBreak {
                lock_after: should_lock,
            })
        } else {
            true
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

    fn disable_for(&mut self, duration: Duration) -> bool {
        if !self.disable_scheduler() {
            return false;
        }
        self.disable_mode = DisableMode::Timed(duration);
        true
    }

    fn disable_until_restart(&mut self) -> bool {
        if !self.disable_scheduler() {
            return false;
        }
        self.disable_mode = DisableMode::UntilRestart;
        true
    }

    fn enable(&mut self) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
    }

    fn disable_scheduler(&mut self) -> bool {
        self.current_break = None;

        if self.scheduler.disable() {
            self.handle_command(BackendCommand::ClearBreak)
        } else {
            true
        }
    }

    fn handle_command(&mut self, command: BackendCommand) -> bool {
        match self.backend.send_command(command) {
            Ok(()) => true,
            Err(error) => {
                warn!(%error, "failed to send backend command");
                false
            }
        }
    }
}

enum RuntimeInput {
    Backend(RuntimeEvent),
    SyncTransport(SyncTransportEvent),
}

enum SelectedRuntimeInput {
    Backend(Result<RuntimeEvent, flume::RecvError>),
    SyncTransport(Result<SyncTransportEvent, flume::RecvError>),
}

struct RuntimeSync<'a> {
    event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
    broadcaster: &'a dyn SyncEventBroadcaster,
}

impl RuntimeSync<'_> {
    fn new(
        event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
        broadcaster: &dyn SyncEventBroadcaster,
    ) -> RuntimeSync<'_> {
        RuntimeSync {
            event_receiver,
            broadcaster,
        }
    }
}

#[cfg(test)]
impl RuntimeSync<'static> {
    fn inactive() -> Self {
        Self::new(None, &NOOP_SYNC_BROADCASTER)
    }
}

trait SyncEventBroadcaster {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError>;
}

impl SyncEventBroadcaster for SyncTransport {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        self.broadcast_event(event)
    }
}

#[cfg(test)]
struct NoopSyncBroadcaster;

#[cfg(test)]
impl SyncEventBroadcaster for NoopSyncBroadcaster {
    fn broadcast_sync_event(&self, _event: SyncEvent) -> Result<usize, SyncTransportError> {
        Ok(0)
    }
}

#[cfg(test)]
static NOOP_SYNC_BROADCASTER: NoopSyncBroadcaster = NoopSyncBroadcaster;

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
