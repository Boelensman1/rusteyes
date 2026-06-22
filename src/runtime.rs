use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::macos_helper::MacOSHelperBackend;
use crate::scheduler::{BreakOrigin, BreakSchedule, BreakScheduler, ScheduledBreak};
use crate::sync_protocol::{PeerId, SyncCompatibilityFingerprint, SyncEvent};
use crate::sync_transport::{
    PeerRejectionReason, SyncTransport, SyncTransportError, SyncTransportEvent,
};
use crate::ui::{PreBreakNotification, RuntimeUi, UiCommand, UiConfig, UiNotification};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
use std::collections::BTreeSet;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::thread;
use std::time::Duration;
use tracing::{trace, warn};

const DEFAULT_PRE_BREAK_NOTICE_LEAD: Duration = Duration::from_secs(30);
const PRE_BREAK_NOTICE_UPDATE_INTERVAL: u64 = 5;

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let sync_compatibility = sync_compatibility_fingerprint(&config)?;
    let Config {
        breaks,
        disable_presets,
        lock,
        sync,
    } = config;
    let ui_config = UiConfig::from_config(&breaks, &disable_presets);
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync, sync_compatibility)?;
    let backend = X11ActivityBackend::spawn(lock)?;

    run_with_ui(schedule, backend, sync_transport, ui_config)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let sync_compatibility = sync_compatibility_fingerprint(&config)?;
    let Config {
        breaks,
        disable_presets,
        lock,
        sync,
    } = config;
    let ui_config = UiConfig::from_config(&breaks, &disable_presets);
    let schedule = BreakSchedule::try_from(breaks)?;
    let sync_transport = SyncTransport::start(sync, sync_compatibility)?;
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
            .name(String::from("rusteyes-runtime"))
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sync_compatibility_fingerprint(
    config: &Config,
) -> Result<Option<SyncCompatibilityFingerprint>, SyncTransportError> {
    if !config.sync.enabled {
        return Ok(None);
    }

    let Some(shared_secret) = &config.sync.shared_secret else {
        return Err(SyncTransportError::MissingSharedSecret);
    };

    SyncCompatibilityFingerprint::from_config(config, shared_secret)
        .map(Some)
        .map_err(SyncTransportError::Protocol)
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
    combined_activity: CombinedActivity,
    idle_reset: IdleReset,
    current_break: Option<CurrentBreakState>,
    pre_break_notice: Option<PreBreakNoticeState>,
    notified_rejected_peers: BTreeSet<PeerId>,
    displayed_active_time: Duration,
}

impl<'a> DaemonRuntime<'a> {
    fn new(
        schedule: BreakSchedule,
        backend: BackendActor,
        sync_runtime: RuntimeSync<'a>,
        ui: RuntimeUi,
    ) -> Self {
        let backend_event_receiver = backend.clone_event_receiver();
        let idle_reset = IdleReset::new(schedule.reset_after_idle());

        Self {
            scheduler: BreakScheduler::new(schedule),
            backend,
            backend_event_receiver,
            sync_event_receiver: sync_runtime.event_receiver,
            sync_broadcaster: sync_runtime.broadcaster,
            ui,
            disable_mode: DisableMode::Enabled,
            combined_activity: CombinedActivity::default(),
            idle_reset,
            current_break: None,
            pre_break_notice: None,
            notified_rejected_peers: BTreeSet::new(),
            displayed_active_time: Duration::ZERO,
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
                return self.observe_active(elapsed, SyncPropagation::Broadcast);
            }
            RuntimeEvent::IdleTimeElapsed(elapsed) => self.advance_idle(elapsed),
            RuntimeEvent::WallClockElapsed(elapsed) => return self.advance_wall_clock(elapsed),
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
            RuntimeEvent::Shutdown => {
                self.clear_pre_break_notice();
                return false;
            }
        }

        true
    }

    fn handle_sync_transport_event(&mut self, event: SyncTransportEvent) -> bool {
        match event {
            SyncTransportEvent::Domain { peer_id, event } => self.handle_sync_event(peer_id, event),
            SyncTransportEvent::PeerRejected { peer_id, reason } => {
                self.notify_peer_rejected(peer_id, reason);
                true
            }
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
                self.observe_active(elapsed, SyncPropagation::Suppress)
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

    fn observe_active(&mut self, elapsed: Duration, propagation: SyncPropagation) -> bool {
        self.idle_reset.reset();
        self.broadcast_if_needed(propagation, &SyncEvent::ActiveTimeElapsed { elapsed });

        let elapsed = self.combined_activity.active_elapsed(elapsed);
        if elapsed.is_zero() {
            return true;
        }

        self.advance_active(elapsed, propagation)
    }

    fn advance_active(&mut self, elapsed: Duration, propagation: SyncPropagation) -> bool {
        let scheduled_break = self.scheduler.advance_active(elapsed);
        if scheduled_break.is_some() {
            self.clear_pre_break_notice();
        }
        self.update_active_time_display();

        if let Some(scheduled_break) = scheduled_break {
            self.start_break(scheduled_break, propagation)
        } else {
            self.notify_upcoming_break();
            true
        }
    }

    fn advance_idle(&mut self, elapsed: Duration) {
        if !self.idle_reset.advance(elapsed) {
            return;
        }

        self.scheduler.reset_active_time();
        self.clear_pre_break_notice();
        self.update_active_time_display();
    }

    fn start_manual_break(&mut self, name: &str, propagation: SyncPropagation) -> bool {
        if let Some(scheduled_break) = self.scheduler.start_manual_break(name) {
            self.update_active_time_display();
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
            self.update_active_time_display();
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

    fn advance_wall_clock(&mut self, elapsed: Duration) -> bool {
        self.combined_activity.advance_wall_clock(elapsed);

        match self.disable_mode {
            DisableMode::Timed(remaining) if elapsed >= remaining => {
                self.enable(SyncPropagation::Suppress);
            }
            DisableMode::Timed(remaining) => {
                self.disable_mode = DisableMode::Timed(remaining.saturating_sub(elapsed));
            }
            DisableMode::Enabled | DisableMode::UntilRestart => {}
        }

        true
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
        self.update_active_time_display();
        self.broadcast_if_needed(propagation, &SyncEvent::Enable);
    }

    fn disable_scheduler(&mut self) -> bool {
        self.current_break = None;
        self.clear_pre_break_notice();

        if self.scheduler.disable() {
            self.update_active_time_display();
            self.handle_command(BackendCommand::ClearBreak)
        } else {
            self.update_active_time_display();
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

        let Some(starts_after) =
            self.next_pre_break_notice_starts_after(&notified_break, upcoming_break.starts_after)
        else {
            return;
        };

        let command = UiCommand::ShowPreBreakNotification(PreBreakNotification {
            break_name: upcoming_break.scheduled_break.name,
            starts_after,
        });

        if let Err(error) = self.ui.send_command(command) {
            warn!(%error, "failed to send pre-break notification command");
        }
        self.pre_break_notice = Some(PreBreakNoticeState {
            notified_break,
            starts_after,
        });
    }

    fn next_pre_break_notice_starts_after(
        &self,
        notified_break: &NotifiedBreak,
        starts_after: Duration,
    ) -> Option<Duration> {
        let Some(pre_break_notice) = &self.pre_break_notice else {
            return Some(starts_after);
        };

        if &pre_break_notice.notified_break != notified_break {
            return Some(starts_after);
        }

        let next_update = next_pre_break_notice_update(pre_break_notice.starts_after)?;

        if starts_after > next_update {
            return None;
        }

        Some(std::cmp::min(starts_after, next_update))
    }

    fn pre_break_notice_lead(&self) -> Duration {
        std::cmp::min(
            DEFAULT_PRE_BREAK_NOTICE_LEAD,
            self.scheduler.after_active() / 2,
        )
    }

    fn clear_pre_break_notice(&mut self) {
        if self.pre_break_notice.take().is_none() {
            return;
        }

        if let Err(error) = self.ui.send_command(UiCommand::ClearPreBreakNotification) {
            warn!(%error, "failed to send pre-break notification clear command");
        }
    }

    fn update_active_time_display(&mut self) {
        let active_time = self.scheduler.active_elapsed();

        if self.displayed_active_time == active_time {
            return;
        }

        self.displayed_active_time = active_time;
        if let Err(error) = self
            .ui
            .send_command(UiCommand::UpdateActiveTime(active_time))
        {
            warn!(%error, "failed to send active-time UI update");
        }
    }

    fn notify_peer_rejected(&mut self, peer_id: PeerId, reason: PeerRejectionReason) {
        if !self.notified_rejected_peers.insert(peer_id) {
            return;
        }

        let body = match reason {
            PeerRejectionReason::IncompatibleConfiguration => format!(
                "Peer {} was rejected because its break settings do not match.",
                short_peer_id(peer_id)
            ),
        };
        let command = UiCommand::ShowNotification(UiNotification {
            summary: String::from("RustEyes sync peer rejected"),
            body,
        });

        if let Err(error) = self.ui.send_command(command) {
            warn!(%error, "failed to send sync peer rejection notification command");
        }
    }
}

fn short_peer_id(peer_id: PeerId) -> String {
    let mut short = peer_id.to_string().chars().take(8).collect::<String>();
    short.push_str("...");
    short
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CombinedActivity {
    active_budget: Duration,
    wall_clock_seen: bool,
}

impl CombinedActivity {
    fn advance_wall_clock(&mut self, elapsed: Duration) {
        self.wall_clock_seen = true;
        self.active_budget = elapsed;
    }

    fn active_elapsed(&mut self, elapsed: Duration) -> Duration {
        if !self.wall_clock_seen {
            return elapsed;
        }

        let elapsed = std::cmp::min(elapsed, self.active_budget);
        self.active_budget -= elapsed;
        elapsed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IdleReset {
    timeout: Option<Duration>,
    idle_elapsed: Duration,
    reset_triggered: bool,
}

impl IdleReset {
    const fn new(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            idle_elapsed: Duration::ZERO,
            reset_triggered: false,
        }
    }

    fn reset(&mut self) {
        self.idle_elapsed = Duration::ZERO;
        self.reset_triggered = false;
    }

    fn advance(&mut self, elapsed: Duration) -> bool {
        let Some(timeout) = self.timeout else {
            return false;
        };

        if self.reset_triggered {
            return false;
        }

        self.idle_elapsed = self.idle_elapsed.saturating_add(elapsed);
        if self.idle_elapsed < timeout {
            return false;
        }

        self.reset_triggered = true;
        true
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
struct PreBreakNoticeState {
    notified_break: NotifiedBreak,
    starts_after: Duration,
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

fn next_pre_break_notice_update(starts_after: Duration) -> Option<Duration> {
    let starts_after_secs = starts_after.as_secs();
    let next_update_secs = if starts_after_secs.is_multiple_of(PRE_BREAK_NOTICE_UPDATE_INTERVAL) {
        starts_after_secs.saturating_sub(PRE_BREAK_NOTICE_UPDATE_INTERVAL)
    } else {
        (starts_after_secs / PRE_BREAK_NOTICE_UPDATE_INTERVAL) * PRE_BREAK_NOTICE_UPDATE_INTERVAL
    };

    (next_update_secs > 0).then(|| Duration::from_secs(next_update_secs))
}

#[cfg(test)]
mod tests;
