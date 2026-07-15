use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::macos_helper::MacOSHelperBackend;
use crate::scheduler::{
    BreakOrigin, BreakSchedule, BreakScheduler, ScheduledBreak, SchedulerPosition,
};
use crate::sync_protocol::{
    PeerId, SyncActiveBreak, SyncBreakOrigin, SyncCompatibilityFingerprint, SyncEvent,
    SyncSchedulerPosition,
};
use crate::sync_transport::{
    PeerRejectionReason, SyncTransport, SyncTransportError, SyncTransportEvent,
};
use crate::ui::{
    PreBreakNotification, RuntimeUi, StatusDisplay, UiCommand, UiConfig, UiNotification,
};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{trace, warn};

const DEFAULT_PRE_BREAK_NOTICE_LEAD: Duration = Duration::from_secs(30);
const FINAL_PRE_BREAK_NOTICE_LEAD: Duration = Duration::from_secs(5);

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let sync_compatibility = sync_compatibility_fingerprint(&config)?;
    let Config {
        breaks,
        disable_presets,
        lock,
        startup: _startup,
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
    crate::macos_login_item::apply_config(config.startup);
    let sync_compatibility = sync_compatibility_fingerprint(&config)?;
    let Config {
        breaks,
        disable_presets,
        lock,
        startup: _startup,
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
                let sync_runtime = RuntimeSync::new(
                    sync_transport.event_receiver(),
                    sync_transport.local_peer_id(),
                    &sync_transport,
                );
                run_with_event_sources(schedule, backend, sync_runtime, ui_runtime, Clock::System);
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
    clock: Clock,
) {
    let mut daemon = DaemonRuntime::new(schedule, backend, sync_runtime, ui_runtime, clock);
    daemon.run();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisableMode {
    Enabled,
    Timed { ends_at_ms: u64 },
    UntilRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncPropagation {
    Broadcast,
    Suppress,
}

struct SyncedBreakStart {
    name: String,
    message: String,
    started_at_ms: u64,
    origin: SyncBreakOrigin,
    position: SyncSchedulerPosition,
    lock_after: Option<bool>,
}

impl SyncPropagation {
    const fn should_broadcast(self) -> bool {
        matches!(self, Self::Broadcast)
    }
}

/// Source of wall-clock time used to stamp and compare break starts across
/// synced peers. Production reads the system clock; tests inject a fixed value
/// so broadcast timestamps and replacement remaining times are deterministic.
enum Clock {
    System,
    #[cfg(test)]
    Fixed(u64),
    #[cfg(test)]
    Shared(Arc<AtomicU64>),
}

impl Clock {
    fn now_unix_millis(&self) -> u64 {
        match self {
            Self::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
                .unwrap_or(0),
            #[cfg(test)]
            Self::Fixed(millis) => *millis,
            #[cfg(test)]
            Self::Shared(millis) => millis.load(AtomicOrdering::Relaxed),
        }
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
    active_time_idle_reset: IdleReset,
    break_count_idle_reset: IdleReset,
    current_break: Option<CurrentBreakState>,
    pre_break_notice: Option<PreBreakNoticeState>,
    notified_rejected_peers: BTreeSet<PeerId>,
    displayed_status: Option<StatusDisplay>,
    displayed_manual_break_availability: BTreeMap<String, bool>,
    local_peer_id: Option<PeerId>,
    clock: Clock,
}

impl<'a> DaemonRuntime<'a> {
    fn new(
        schedule: BreakSchedule,
        backend: BackendActor,
        sync_runtime: RuntimeSync<'a>,
        ui: RuntimeUi,
        clock: Clock,
    ) -> Self {
        let backend_event_receiver = backend.clone_event_receiver();
        let active_time_idle_reset = IdleReset::new(schedule.reset_after_idle());
        let break_count_idle_reset = IdleReset::new(schedule.reset_count_after_idle());
        let scheduler = BreakScheduler::new(schedule);
        let displayed_manual_break_availability = scheduler.manual_break_availability();

        Self {
            scheduler,
            backend,
            backend_event_receiver,
            sync_event_receiver: sync_runtime.event_receiver,
            sync_broadcaster: sync_runtime.broadcaster,
            ui,
            disable_mode: DisableMode::Enabled,
            combined_activity: CombinedActivity::default(),
            active_time_idle_reset,
            break_count_idle_reset,
            current_break: None,
            pre_break_notice: None,
            notified_rejected_peers: BTreeSet::new(),
            displayed_status: None,
            displayed_manual_break_availability,
            local_peer_id: sync_runtime.local_peer_id,
            clock,
        }
    }

    fn run(&mut self) {
        self.update_status_display();

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
                return self.observe_active(
                    elapsed,
                    SyncPropagation::Broadcast,
                    SyncPropagation::Broadcast,
                );
            }
            RuntimeEvent::IdleTimeElapsed(elapsed) => self.advance_idle(elapsed),
            RuntimeEvent::WallClockElapsed(elapsed) => return self.advance_wall_clock(elapsed),
            RuntimeEvent::BreakStartFailed => return self.break_start_failed(),
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
            SyncTransportEvent::PeerAuthenticated(peer_id) => {
                self.send_scheduler_state_to_peer(peer_id);
                true
            }
            SyncTransportEvent::Domain { peer_id, event } => self.handle_sync_event(peer_id, event),
            SyncTransportEvent::PeerRejected { peer_id, reason } => {
                self.notify_peer_rejected(peer_id, reason);
                true
            }
            SyncTransportEvent::PeerDisconnected(peer_id) => {
                trace!(peer_id = %peer_id, "sync peer disconnected");
                true
            }
        }
    }

    fn handle_sync_event(&mut self, peer_id: PeerId, event: SyncEvent) -> bool {
        match event {
            SyncEvent::ActiveTimeElapsed { elapsed } => {
                trace!(peer_id = %peer_id, ?elapsed, "applying synced active time");
                self.observe_active(
                    elapsed,
                    SyncPropagation::Suppress,
                    SyncPropagation::Broadcast,
                )
            }
            SyncEvent::BreakStarted {
                name,
                message,
                started_at_ms,
                origin,
                position,
            } => {
                trace!(peer_id = %peer_id, break_name = %name, "applying synced break start");
                self.apply_synced_break_start(
                    peer_id,
                    SyncedBreakStart {
                        name,
                        message,
                        started_at_ms,
                        origin,
                        position,
                        lock_after: None,
                    },
                )
            }
            SyncEvent::SchedulerState {
                slot,
                active_elapsed,
                last_satisfied_slots,
                active_break,
            } => {
                trace!(
                    peer_id = %peer_id,
                    slot,
                    ?active_elapsed,
                    "applying synced scheduler state"
                );
                self.apply_synced_scheduler_state(
                    peer_id,
                    slot,
                    active_elapsed,
                    last_satisfied_slots,
                    active_break,
                )
            }
            SyncEvent::SchedulerReset => {
                trace!(peer_id = %peer_id, "applying synced scheduler reset");
                self.apply_scheduler_reset(SyncPropagation::Suppress);
                true
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

    fn send_sync_event(&self, peer_id: PeerId, event: &SyncEvent) {
        let result = self
            .sync_broadcaster
            .send_sync_event(peer_id, event.clone());

        match result {
            Ok(true) => {
                trace!(peer_id = %peer_id, ?event, "sent directed sync event");
            }
            Ok(false) => {
                trace!(peer_id = %peer_id, ?event, "directed sync peer is no longer connected");
            }
            Err(error) => {
                warn!(%error, peer_id = %peer_id, ?event, "failed to send directed sync event");
            }
        }
    }

    fn send_scheduler_state_to_peer(&self, peer_id: PeerId) {
        self.send_sync_event(peer_id, &self.scheduler_state_event());
    }

    fn scheduler_state_event(&self) -> SyncEvent {
        let SchedulerPosition {
            slot,
            active_elapsed,
            last_satisfied_slots,
        } = self.scheduler.position();

        SyncEvent::SchedulerState {
            slot,
            active_elapsed,
            last_satisfied_slots,
            active_break: self
                .current_break
                .as_ref()
                .map(CurrentBreakState::to_sync_active_break),
        }
    }

    fn observe_active(
        &mut self,
        elapsed: Duration,
        active_time_propagation: SyncPropagation,
        break_start_propagation: SyncPropagation,
    ) -> bool {
        self.reset_idle_tracking();
        self.broadcast_if_needed(
            active_time_propagation,
            &SyncEvent::ActiveTimeElapsed { elapsed },
        );

        let elapsed = self.combined_activity.active_elapsed(elapsed);
        if elapsed.is_zero() {
            return true;
        }

        self.advance_active(elapsed, break_start_propagation)
    }

    fn advance_active(&mut self, elapsed: Duration, propagation: SyncPropagation) -> bool {
        let scheduled_break = self.scheduler.advance_active(elapsed);
        if scheduled_break.is_some() {
            self.clear_pre_break_notice();
        }
        self.update_status_display();

        if let Some(scheduled_break) = scheduled_break {
            self.start_break(scheduled_break, propagation)
        } else {
            self.notify_upcoming_break();
            true
        }
    }

    fn advance_idle(&mut self, elapsed: Duration) {
        let reset_count = self.break_count_idle_reset.advance(elapsed);
        let reset_active_time = self.active_time_idle_reset.advance(elapsed);

        if reset_count {
            self.apply_scheduler_reset(SyncPropagation::Broadcast);
        } else if reset_active_time {
            self.scheduler.reset_active_time();
            self.clear_pre_break_notice();
            self.update_status_display();
        }
    }

    fn apply_scheduler_reset(&mut self, propagation: SyncPropagation) {
        self.active_time_idle_reset.mark_triggered();
        self.break_count_idle_reset.mark_triggered();
        let changed = self.scheduler.reset_position();
        self.clear_pre_break_notice();
        self.update_status_display();

        if changed {
            self.broadcast_if_needed(propagation, &SyncEvent::SchedulerReset);
        }
    }

    fn reset_idle_tracking(&mut self) {
        self.active_time_idle_reset.reset();
        self.break_count_idle_reset.reset();
    }

    fn start_manual_break(&mut self, name: &str, propagation: SyncPropagation) -> bool {
        self.start_named_break(name, None, None, propagation)
    }

    /// Starts a configured break by name, optionally overriding the displayed
    /// message and the start timestamp. Synced break starts pass the peer's
    /// values so every machine shows the same message and shares one timeline.
    fn start_named_break(
        &mut self,
        name: &str,
        message: Option<String>,
        started_at_ms: Option<u64>,
        propagation: SyncPropagation,
    ) -> bool {
        let Some(mut scheduled_break) = self.scheduler.start_manual_break(name) else {
            return true;
        };
        if let Some(message) = message {
            scheduled_break.message = message;
        }
        self.update_status_display();
        let started_at_ms = started_at_ms.unwrap_or_else(|| self.clock.now_unix_millis());
        self.start_break_at(scheduled_break, started_at_ms, propagation)
    }

    fn start_synced_break(
        &mut self,
        name: &str,
        origin: BreakOrigin,
        message: String,
        started_at_ms: u64,
        lock_after: Option<bool>,
        position: SchedulerPosition,
    ) -> bool {
        let Some(mut scheduled_break) = self.scheduler.start_active_synced_break(name, origin)
        else {
            return true;
        };
        self.scheduler.merge_synced_position(position);

        scheduled_break.message = message;
        if let Some(lock_after) = lock_after {
            scheduled_break.autolock = lock_after;
        }

        self.update_status_display();
        self.start_break_at(scheduled_break, started_at_ms, SyncPropagation::Suppress)
    }

    fn start_break(
        &mut self,
        scheduled_break: ScheduledBreak,
        propagation: SyncPropagation,
    ) -> bool {
        let started_at_ms = self.clock.now_unix_millis();
        self.start_break_at(scheduled_break, started_at_ms, propagation)
    }

    fn start_break_at(
        &mut self,
        mut scheduled_break: ScheduledBreak,
        started_at_ms: u64,
        propagation: SyncPropagation,
    ) -> bool {
        let state_break = scheduled_break.clone();
        let remaining = self.remaining_break_duration(started_at_ms, scheduled_break.duration);
        if remaining.is_zero() {
            if self.scheduler.finish_break() {
                self.update_status_display();
            }
            return true;
        }

        scheduled_break.duration = remaining;
        let name = scheduled_break.name.clone();
        let message = scheduled_break.message.clone();
        let origin = sync_origin_from_break(&scheduled_break.origin);
        let position = sync_position_from_scheduler(self.scheduler.position());
        self.clear_pre_break_notice();
        self.current_break = Some(CurrentBreakState::for_break(&state_break, started_at_ms));

        if self.handle_command(BackendCommand::StartBreak(scheduled_break)) {
            self.broadcast_if_needed(
                propagation,
                &SyncEvent::BreakStarted {
                    name,
                    message,
                    started_at_ms,
                    origin,
                    position,
                },
            );
            true
        } else {
            false
        }
    }

    fn remaining_break_duration(&self, started_at_ms: u64, duration: Duration) -> Duration {
        let now = self.clock.now_unix_millis();
        let elapsed = Duration::from_millis(now.saturating_sub(started_at_ms));

        duration.saturating_sub(elapsed)
    }

    /// Applies a break start received from a peer. The peer's scheduler
    /// position carries any cadence resets caused by the break start. If a
    /// break is already showing, newer scheduled slots replace older scheduled
    /// slots, while same-slot/manual collisions keep the existing timestamp
    /// ordering.
    fn apply_synced_break_start(&mut self, peer_id: PeerId, start: SyncedBreakStart) -> bool {
        let SyncedBreakStart {
            name,
            message,
            started_at_ms,
            origin,
            position,
            lock_after,
        } = start;
        let Some(origin) = break_origin_from_sync(origin) else {
            return true;
        };
        if !self.scheduler.has_break(&name) {
            return true;
        }
        let position = scheduler_position_from_sync(position);

        let Some(current) = self.current_break.clone() else {
            return self.start_synced_break(
                &name,
                origin,
                message,
                started_at_ms,
                lock_after,
                position,
            );
        };

        if self.synced_break_replaces_current(&current, peer_id, origin, started_at_ms) {
            let Some(mut scheduled_break) = self.scheduler.replacement_synced_break(&name, origin)
            else {
                return true;
            };
            scheduled_break.message = message;
            if let Some(lock_after) = lock_after {
                scheduled_break.autolock = lock_after;
            } else if current.lock_after() {
                scheduled_break.autolock = true;
            }
            self.scheduler.merge_synced_position(position);
            self.replace_current_break(&scheduled_break, started_at_ms)
        } else {
            if self.scheduler.merge_synced_position(position) {
                self.update_status_display();
            }
            true
        }
    }

    fn synced_break_replaces_current(
        &self,
        current: &CurrentBreakState,
        peer_id: PeerId,
        peer_origin: BreakOrigin,
        peer_started_ms: u64,
    ) -> bool {
        match (peer_origin, current.origin()) {
            (
                BreakOrigin::Scheduled { slot: peer_slot },
                BreakOrigin::Scheduled { slot: current_slot },
            ) if peer_slot != current_slot => return peer_slot > current_slot,
            _ => {}
        }

        match peer_started_ms.cmp(&current.started_at_ms()) {
            Ordering::Less => true,
            Ordering::Greater => false,
            Ordering::Equal => self.local_peer_id.is_some_and(|local| peer_id < local),
        }
    }

    fn replace_current_break(
        &mut self,
        scheduled_break: &ScheduledBreak,
        started_at_ms: u64,
    ) -> bool {
        let remaining = self.remaining_break_duration(started_at_ms, scheduled_break.duration);
        if remaining.is_zero() {
            self.current_break = None;
            if self.scheduler.finish_break() {
                self.update_status_display();
            }
            return self.handle_command(BackendCommand::ClearBreak);
        }

        self.current_break = Some(CurrentBreakState::for_break(scheduled_break, started_at_ms));
        let message = scheduled_break.message.clone();

        self.handle_command(BackendCommand::ReplaceActiveBreak {
            message,
            remaining,
            lock_after: scheduled_break.autolock,
        })
    }

    fn apply_synced_scheduler_state(
        &mut self,
        peer_id: PeerId,
        slot: usize,
        active_elapsed: Duration,
        last_satisfied_slots: BTreeMap<String, usize>,
        active_break: Option<SyncActiveBreak>,
    ) -> bool {
        let position = SyncSchedulerPosition {
            slot,
            active_elapsed,
            last_satisfied_slots,
        };

        if let Some(active_break) = active_break
            && !self.apply_synced_break_start(
                peer_id,
                SyncedBreakStart {
                    name: active_break.name,
                    message: active_break.message,
                    started_at_ms: active_break.started_at_ms,
                    origin: active_break.origin,
                    position: position.clone(),
                    lock_after: Some(active_break.lock_after),
                },
            )
        {
            return false;
        }

        if self
            .scheduler
            .merge_synced_position(scheduler_position_from_sync(position))
        {
            self.update_status_display();
        }

        true
    }

    fn break_start_failed(&mut self) -> bool {
        self.clear_pre_break_notice();
        self.current_break = None;

        if self.scheduler.finish_break() {
            self.update_status_display();
        }

        true
    }

    fn finish_break(&mut self) -> bool {
        self.clear_pre_break_notice();

        // Only emit `FinishBreak` when the runtime believed a break overlay was
        // showing. The early return also makes a duplicate `BreakFinished` a
        // no-op so we never send a second finish (which would spuriously lock or
        // beep again). Crucially, the overlay teardown is gated on
        // `current_break`, NOT on `scheduler.finish_break()`: the scheduler can
        // have left `Pending` while the overlay is still up, and we must clear it
        // regardless so the input-blocking event tap is always released.
        let Some(current_break) = self.current_break.take() else {
            return true;
        };
        let should_lock = current_break.lock_after();

        if self.scheduler.finish_break() {
            self.update_status_display();
        }

        self.handle_command(BackendCommand::FinishBreak {
            lock_after: should_lock,
        })
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

        if let DisableMode::Timed { ends_at_ms } = self.disable_mode
            && self.clock.now_unix_millis() >= ends_at_ms
        {
            self.enable(SyncPropagation::Suppress);
        }

        self.update_status_display();
        true
    }

    fn disable_for(&mut self, duration: Duration, propagation: SyncPropagation) -> bool {
        // Set the mode before disabling the scheduler so the status line the
        // scheduler refresh emits reflects the new disabled countdown.
        let ends_at_ms = self
            .clock
            .now_unix_millis()
            .saturating_add(duration_millis_u64(duration));
        self.disable_mode = DisableMode::Timed { ends_at_ms };
        if !self.disable_scheduler() {
            return false;
        }
        self.broadcast_if_needed(propagation, &SyncEvent::DisableFor { duration });
        true
    }

    fn disable_until_restart(&mut self, propagation: SyncPropagation) -> bool {
        self.disable_mode = DisableMode::UntilRestart;
        if !self.disable_scheduler() {
            return false;
        }
        self.broadcast_if_needed(propagation, &SyncEvent::DisableUntilRestart);
        true
    }

    fn enable(&mut self, propagation: SyncPropagation) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
        self.update_status_display();
        self.broadcast_if_needed(propagation, &SyncEvent::Enable);
    }

    fn disable_scheduler(&mut self) -> bool {
        self.current_break = None;
        self.clear_pre_break_notice();

        if self.scheduler.disable() {
            self.update_status_display();
            self.handle_command(BackendCommand::ClearBreak)
        } else {
            self.update_status_display();
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

    fn update_status_display(&mut self) {
        let status = match self.disable_mode {
            DisableMode::Enabled => self.scheduler.upcoming_scheduled_break().map(|upcoming| {
                StatusDisplay::UpcomingBreak {
                    break_name: upcoming.scheduled_break.name,
                    starts_after: upcoming.starts_after,
                }
            }),
            DisableMode::Timed { ends_at_ms } => Some(StatusDisplay::DisabledFor(
                self.timed_disable_remaining(ends_at_ms),
            )),
            DisableMode::UntilRestart => Some(StatusDisplay::DisabledUntilRestart),
        };

        if let Some(status) = status
            && self.displayed_status.as_ref() != Some(&status)
        {
            self.displayed_status = Some(status.clone());
            if let Err(error) = self.ui.send_command(UiCommand::UpdateStatus(status)) {
                warn!(%error, "failed to send status UI update");
            }
        }

        self.update_manual_break_availability();
    }

    fn update_manual_break_availability(&mut self) {
        let availability = self.scheduler.manual_break_availability();
        if self.displayed_manual_break_availability == availability {
            return;
        }

        self.displayed_manual_break_availability = availability.clone();
        if let Err(error) = self
            .ui
            .send_command(UiCommand::UpdateManualBreakAvailability(availability))
        {
            warn!(%error, "failed to send manual break availability UI update");
        }
    }

    fn timed_disable_remaining(&self, ends_at_ms: u64) -> Duration {
        let remaining_ms = ends_at_ms.saturating_sub(self.clock.now_unix_millis());

        Duration::from_millis(remaining_ms)
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

fn sync_origin_from_break(origin: &BreakOrigin) -> SyncBreakOrigin {
    match origin {
        BreakOrigin::Scheduled { slot } => SyncBreakOrigin::Scheduled { slot: *slot },
        BreakOrigin::Manual => SyncBreakOrigin::Manual,
    }
}

fn break_origin_from_sync(origin: SyncBreakOrigin) -> Option<BreakOrigin> {
    match origin {
        SyncBreakOrigin::Manual => Some(BreakOrigin::Manual),
        SyncBreakOrigin::Scheduled { slot } if slot > 0 => Some(BreakOrigin::Scheduled { slot }),
        SyncBreakOrigin::Scheduled { .. } => None,
    }
}

fn sync_position_from_scheduler(position: SchedulerPosition) -> SyncSchedulerPosition {
    SyncSchedulerPosition {
        slot: position.slot,
        active_elapsed: position.active_elapsed,
        last_satisfied_slots: position.last_satisfied_slots,
    }
}

fn scheduler_position_from_sync(position: SyncSchedulerPosition) -> SchedulerPosition {
    SchedulerPosition {
        slot: position.slot,
        active_elapsed: position.active_elapsed,
        last_satisfied_slots: position.last_satisfied_slots,
    }
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

    fn mark_triggered(&mut self) {
        self.reset_triggered = true;
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
    local_peer_id: Option<PeerId>,
    broadcaster: &'a dyn SyncEventBroadcaster,
}

impl RuntimeSync<'_> {
    fn new(
        event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
        local_peer_id: Option<PeerId>,
        broadcaster: &dyn SyncEventBroadcaster,
    ) -> RuntimeSync<'_> {
        RuntimeSync {
            event_receiver,
            local_peer_id,
            broadcaster,
        }
    }
}

#[cfg(test)]
impl RuntimeSync<'static> {
    fn inactive() -> Self {
        Self::new(None, None, &NOOP_SYNC_BROADCASTER)
    }
}

trait SyncEventBroadcaster {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError>;
    fn send_sync_event(
        &self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError>;
}

impl SyncEventBroadcaster for SyncTransport {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        self.broadcast_event(event)
    }

    fn send_sync_event(
        &self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        self.send_event(peer_id, event)
    }
}

#[cfg(test)]
struct NoopSyncBroadcaster;

#[cfg(test)]
impl SyncEventBroadcaster for NoopSyncBroadcaster {
    fn broadcast_sync_event(&self, _event: SyncEvent) -> Result<usize, SyncTransportError> {
        Ok(0)
    }

    fn send_sync_event(
        &self,
        _peer_id: PeerId,
        _event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        Ok(false)
    }
}

#[cfg(test)]
static NOOP_SYNC_BROADCASTER: NoopSyncBroadcaster = NoopSyncBroadcaster;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentBreakState {
    name: String,
    message: String,
    origin: BreakOrigin,
    lock_after: bool,
    started_at_ms: u64,
}

impl CurrentBreakState {
    fn for_break(scheduled_break: &ScheduledBreak, started_at_ms: u64) -> Self {
        Self {
            name: scheduled_break.name.clone(),
            message: scheduled_break.message.clone(),
            origin: scheduled_break.origin,
            lock_after: scheduled_break.autolock,
            started_at_ms,
        }
    }

    fn request_lock_after(&mut self) -> bool {
        if self.lock_after {
            return false;
        }

        self.lock_after = true;
        true
    }

    const fn lock_after(&self) -> bool {
        self.lock_after
    }

    const fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    const fn origin(&self) -> BreakOrigin {
        self.origin
    }

    fn to_sync_active_break(&self) -> SyncActiveBreak {
        SyncActiveBreak {
            name: self.name.clone(),
            message: self.message.clone(),
            started_at_ms: self.started_at_ms,
            origin: sync_origin_from_break(&self.origin),
            lock_after: self.lock_after,
        }
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
    (starts_after > FINAL_PRE_BREAK_NOTICE_LEAD).then_some(FINAL_PRE_BREAK_NOTICE_LEAD)
}

#[cfg(test)]
mod tests;
