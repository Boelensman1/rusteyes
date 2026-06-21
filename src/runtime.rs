use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::macos_helper::MacOSHelperBackend;
use crate::scheduler::{BreakOrigin, BreakSchedule, BreakScheduler, ScheduledBreak};
use crate::sync_protocol::{PeerId, SyncEvent};
use crate::sync_transport::{SyncTransport, SyncTransportError, SyncTransportEvent};
use crate::ui::{PreBreakNotification, RuntimeUi, UiCommand, UiConfig};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::thread;
use std::time::Duration;
use tracing::{trace, warn};

const DEFAULT_PRE_BREAK_NOTICE_LEAD: Duration = Duration::from_secs(30);

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config {
        breaks,
        disable_presets,
        lock,
        sync,
    } = Config::load()?;
    let ui_config = UiConfig::from_config(&breaks, &disable_presets);
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync)?;
    let backend = X11ActivityBackend::spawn(lock)?;

    run_with_ui(schedule, backend, sync_transport, ui_config)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config {
        breaks,
        disable_presets,
        lock,
        sync,
    } = Config::load()?;
    let ui_config = UiConfig::from_config(&breaks, &disable_presets);
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync)?;
    let backend = MacOSHelperBackend::spawn(lock)?;

    run_with_ui(schedule, backend, sync_transport, ui_config)?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn run() -> Result<(), crate::Error> {
    Err(crate::Error::unsupported_platform())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_with_ui(
    schedule: BreakSchedule,
    backend: BackendActor,
    sync_transport: SyncTransport,
    ui_config: UiConfig,
) -> Result<(), crate::Error> {
    crate::ui::run(ui_config, move |ui_proxy, ui_handle| {
        thread::Builder::new()
            .name(String::from("resteyes-runtime"))
            .spawn(move || {
                let ui_runtime = crate::ui::runtime_ui_from_handle(ui_handle);
                let sync_runtime =
                    RuntimeSync::new(sync_transport.event_receiver(), &sync_transport);
                run_with_event_sources(schedule, backend, sync_runtime, ui_runtime);
                ui_proxy.runtime_stopped();
            })
            .map_err(|error| crate::ui::UiError::runtime_thread(&error))
    })?;
    Ok(())
}

fn run_with_event_sources(
    schedule: BreakSchedule,
    backend: BackendActor,
    sync_runtime: RuntimeSync<'_>,
    ui_runtime: RuntimeUi,
) {
    let mut daemon = DaemonRuntime::new(schedule, backend, sync_runtime, ui_runtime);
    daemon.run();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisableMode {
    Enabled,
    Timed(Duration),
    UntilRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncPropagation {
    Broadcast,
    Suppress,
}

impl SyncPropagation {
    const fn should_broadcast(self) -> bool {
        matches!(self, Self::Broadcast)
    }
}

struct DaemonRuntime<'a> {
    scheduler: BreakScheduler,
    backend: BackendActor,
    backend_event_receiver: flume::Receiver<RuntimeEvent>,
    sync_event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
    sync_broadcaster: &'a dyn SyncEventBroadcaster,
    ui: RuntimeUi,
    disable_mode: DisableMode,
    current_break: Option<CurrentBreakState>,
    notified_break: Option<NotifiedBreak>,
}

impl<'a> DaemonRuntime<'a> {
    fn new(
        schedule: BreakSchedule,
        backend: BackendActor,
        sync_runtime: RuntimeSync<'a>,
        ui: RuntimeUi,
    ) -> Self {
        let backend_event_receiver = backend.clone_event_receiver();

        Self {
            scheduler: BreakScheduler::new(schedule),
            backend,
            backend_event_receiver,
            sync_event_receiver: sync_runtime.event_receiver,
            sync_broadcaster: sync_runtime.broadcaster,
            ui,
            disable_mode: DisableMode::Enabled,
            current_break: None,
            notified_break: None,
        }
    }

    fn run(&mut self) {
        while let Some(input) = self.next_input() {
            if !self.handle_input(input) {
                break;
            }
        }
    }

    fn next_input(&mut self) -> Option<RuntimeInput> {
        loop {
            let selected = match (&self.sync_event_receiver, self.ui.event_receiver()) {
                (Some(sync_event_receiver), Some(ui_event_receiver)) => flume::Selector::new()
                    .recv(&self.backend_event_receiver, SelectedRuntimeInput::Backend)
                    .recv(sync_event_receiver, SelectedRuntimeInput::SyncTransport)
                    .recv(ui_event_receiver, SelectedRuntimeInput::Ui)
                    .wait(),
                (Some(sync_event_receiver), None) => flume::Selector::new()
                    .recv(&self.backend_event_receiver, SelectedRuntimeInput::Backend)
                    .recv(sync_event_receiver, SelectedRuntimeInput::SyncTransport)
                    .wait(),
                (None, Some(ui_event_receiver)) => flume::Selector::new()
                    .recv(&self.backend_event_receiver, SelectedRuntimeInput::Backend)
                    .recv(ui_event_receiver, SelectedRuntimeInput::Ui)
                    .wait(),
                (None, None) => flume::Selector::new()
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
                SelectedRuntimeInput::Ui(Ok(event)) => {
                    return Some(RuntimeInput::Ui(event));
                }
                SelectedRuntimeInput::Ui(Err(_)) => {
                    warn!("ui event channel closed");
                    self.ui.clear_event_receiver();
                }
            }
        }
    }

    fn handle_input(&mut self, input: RuntimeInput) -> bool {
        match input {
            RuntimeInput::Backend(event) | RuntimeInput::Ui(event) => self.handle_event(event),
            RuntimeInput::SyncTransport(event) => self.handle_sync_transport_event(event),
        }
    }

    fn handle_event(&mut self, event: RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::ActiveTimeElapsed(elapsed) => {
                return self.advance_active(elapsed, SyncPropagation::Broadcast);
            }
            RuntimeEvent::WallClockElapsed(elapsed) => self.advance_wall_clock(elapsed),
            RuntimeEvent::BreakFinished => return self.finish_break(),
            RuntimeEvent::LockAfterCurrentBreak => {
                return self.request_lock_after_current_break(SyncPropagation::Broadcast);
            }
            RuntimeEvent::StartManualBreak(name) => {
                return self.start_manual_break(&name, SyncPropagation::Broadcast);
            }
            RuntimeEvent::Disable(DisableRequest::For(duration)) => {
                return self.disable_for(duration, SyncPropagation::Broadcast);
            }
            RuntimeEvent::Disable(DisableRequest::UntilRestart) => {
                return self.disable_until_restart(SyncPropagation::Broadcast);
            }
            RuntimeEvent::Enable => self.enable(SyncPropagation::Broadcast),
            RuntimeEvent::Shutdown => return false,
        }

        true
    }

    fn handle_sync_transport_event(&mut self, event: SyncTransportEvent) -> bool {
        match event {
            SyncTransportEvent::Domain { peer_id, event } => self.handle_sync_event(peer_id, event),
            event => {
                trace!(?event, "received sync transport event");
                true
            }
        }
    }

    fn handle_sync_event(&mut self, peer_id: PeerId, event: SyncEvent) -> bool {
        match event {
            SyncEvent::ActiveTimeElapsed { elapsed } => {
                trace!(peer_id = %peer_id, ?elapsed, "applying synced active time");
                self.advance_active(elapsed, SyncPropagation::Suppress)
            }
            SyncEvent::BreakStarted { name } => {
                trace!(peer_id = %peer_id, break_name = %name, "applying synced break start");
                self.start_manual_break(&name, SyncPropagation::Suppress)
            }
            SyncEvent::DisableFor { duration } => {
                trace!(peer_id = %peer_id, ?duration, "applying synced timed disable");
                self.disable_for(duration, SyncPropagation::Suppress)
            }
            SyncEvent::DisableUntilRestart => {
                trace!(peer_id = %peer_id, "applying synced disable until restart");
                self.disable_until_restart(SyncPropagation::Suppress)
            }
            SyncEvent::Enable => {
                trace!(peer_id = %peer_id, "applying synced enable");
                self.enable(SyncPropagation::Suppress);
                true
            }
            SyncEvent::LockAfterCurrentBreak => {
                trace!(
                    peer_id = %peer_id,
                    "received synced lock-after-current-break request"
                );
                self.request_lock_after_current_break(SyncPropagation::Suppress)
            }
        }
    }

    fn broadcast_sync_event(&self, event: &SyncEvent) {
        let result = self.sync_broadcaster.broadcast_sync_event(event.clone());

        match result {
            Ok(peer_count) => {
                trace!(?event, peer_count, "broadcast sync event");
            }
            Err(error) => {
                warn!(%error, ?event, "failed to broadcast sync event");
            }
        }
    }

    fn broadcast_if_needed(&self, propagation: SyncPropagation, event: &SyncEvent) {
        if propagation.should_broadcast() {
            self.broadcast_sync_event(event);
        }
    }

    fn advance_active(&mut self, elapsed: Duration, propagation: SyncPropagation) -> bool {
        self.broadcast_if_needed(propagation, &SyncEvent::ActiveTimeElapsed { elapsed });

        if let Some(scheduled_break) = self.scheduler.advance_active(elapsed) {
            self.start_break(scheduled_break, propagation)
        } else {
            self.notify_upcoming_break();
            true
        }
    }

    fn start_manual_break(&mut self, name: &str, propagation: SyncPropagation) -> bool {
        if let Some(scheduled_break) = self.scheduler.start_manual_break(name) {
            self.start_break(scheduled_break, propagation)
        } else {
            true
        }
    }

    fn start_break(
        &mut self,
        scheduled_break: ScheduledBreak,
        propagation: SyncPropagation,
    ) -> bool {
        let name = scheduled_break.name.clone();
        self.clear_pre_break_notice();
        self.current_break = Some(CurrentBreakState::for_break(&scheduled_break));

        if self.handle_command(BackendCommand::StartBreak(scheduled_break)) {
            self.broadcast_if_needed(propagation, &SyncEvent::BreakStarted { name });
            true
        } else {
            false
        }
    }

    fn finish_break(&mut self) -> bool {
        self.clear_pre_break_notice();
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

    fn request_lock_after_current_break(&mut self, propagation: SyncPropagation) -> bool {
        let Some(current_break) = &mut self.current_break else {
            return true;
        };

        if !current_break.request_lock_after() {
            return true;
        }

        if self.handle_command(BackendCommand::RequestLockAfterCurrentBreak) {
            self.broadcast_if_needed(propagation, &SyncEvent::LockAfterCurrentBreak);
            true
        } else {
            false
        }
    }

    fn advance_wall_clock(&mut self, elapsed: Duration) {
        match self.disable_mode {
            DisableMode::Timed(remaining) if elapsed >= remaining => {
                self.enable(SyncPropagation::Suppress);
            }
            DisableMode::Timed(remaining) => {
                self.disable_mode = DisableMode::Timed(remaining.saturating_sub(elapsed));
            }
            DisableMode::Enabled | DisableMode::UntilRestart => {}
        }
    }

    fn disable_for(&mut self, duration: Duration, propagation: SyncPropagation) -> bool {
        if !self.disable_scheduler() {
            return false;
        }
        self.disable_mode = DisableMode::Timed(duration);
        self.broadcast_if_needed(propagation, &SyncEvent::DisableFor { duration });
        true
    }

    fn disable_until_restart(&mut self, propagation: SyncPropagation) -> bool {
        if !self.disable_scheduler() {
            return false;
        }
        self.disable_mode = DisableMode::UntilRestart;
        self.broadcast_if_needed(propagation, &SyncEvent::DisableUntilRestart);
        true
    }

    fn enable(&mut self, propagation: SyncPropagation) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
        self.broadcast_if_needed(propagation, &SyncEvent::Enable);
    }

    fn disable_scheduler(&mut self) -> bool {
        self.current_break = None;
        self.clear_pre_break_notice();

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

    fn notify_upcoming_break(&mut self) {
        let Some(upcoming_break) = self.scheduler.upcoming_scheduled_break() else {
            return;
        };

        if upcoming_break.starts_after > self.pre_break_notice_lead() {
            return;
        }

        let Some(notified_break) =
            NotifiedBreak::from_scheduled_break(&upcoming_break.scheduled_break)
        else {
            return;
        };

        if self.notified_break == Some(notified_break.clone()) {
            return;
        }

        let command = UiCommand::ShowPreBreakNotification(PreBreakNotification {
            break_name: upcoming_break.scheduled_break.name,
            starts_after: upcoming_break.starts_after,
        });

        if let Err(error) = self.ui.send_command(command) {
            warn!(%error, "failed to send pre-break notification command");
        }
        self.notified_break = Some(notified_break);
    }

    fn pre_break_notice_lead(&self) -> Duration {
        std::cmp::min(
            DEFAULT_PRE_BREAK_NOTICE_LEAD,
            self.scheduler.after_active() / 2,
        )
    }

    fn clear_pre_break_notice(&mut self) {
        self.notified_break = None;
    }
}

enum RuntimeInput {
    Backend(RuntimeEvent),
    Ui(RuntimeEvent),
    SyncTransport(SyncTransportEvent),
}

enum SelectedRuntimeInput {
    Backend(Result<RuntimeEvent, flume::RecvError>),
    Ui(Result<RuntimeEvent, flume::RecvError>),
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

    fn request_lock_after(&mut self) -> bool {
        if self.lock_after {
            return false;
        }

        self.lock_after = true;
        true
    }

    const fn lock_after(self) -> bool {
        self.lock_after
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifiedBreak {
    name: String,
    slot: usize,
}

impl NotifiedBreak {
    fn from_scheduled_break(scheduled_break: &ScheduledBreak) -> Option<Self> {
        let BreakOrigin::Scheduled { slot } = scheduled_break.origin else {
            return None;
        };

        Some(Self {
            name: scheduled_break.name.clone(),
            slot,
        })
    }
}

#[cfg(test)]
mod tests;
