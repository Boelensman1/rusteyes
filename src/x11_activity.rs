use crate::activity::{ActivityPoller, ActivitySample, BreakTimer, break_elapsed_for_sample};
use crate::backend::{
    BackendActor, BackendActorSpawnError, BackendCommand, BackendCommandReceiver,
    BackendEventSender, BackendWait, RuntimeEvent, wait_for_command_or_timeout,
};
use crate::config::LockConfig;
use crate::lock_command::{LockCommand, LockCommandError, start_lock_command};
use crate::scheduler::ScheduledBreak;
use crate::x11_overlay::{X11Overlay, X11OverlayError, X11Screen};
use std::fmt;
use std::thread;
use std::time::Duration;
use tracing::{error, trace};
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt;
use x11rb::rust_connection::RustConnection;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const OVERLAY_TICK_INTERVAL: Duration = Duration::from_millis(500);
const BREAK_START_RETRY_TIMEOUT: Duration = Duration::from_secs(10);
const BREAK_FINISHED_BELL_PERCENT: i8 = 0;
const LOCK_HANDOFF_DELAY: Duration = Duration::from_millis(250);
const DEFAULT_LOCK_COMMAND: [&str; 2] = ["loginctl", "lock-session"];
const SCREENSAVER_CLIENT_MAJOR_VERSION: u8 = 1;
const SCREENSAVER_CLIENT_MINOR_VERSION: u8 = 1;

#[allow(clippy::module_name_repetitions)]
pub(crate) struct X11ActivityBackend {
    activity: X11Activity,
    poller: ActivityPoller,
    active_break: Option<ActiveBreak>,
    pending_break: Option<PendingBreakStart>,
    lock_command: LockCommand,
}

impl X11ActivityBackend {
    pub(crate) fn spawn(lock_config: LockConfig) -> Result<BackendActor, X11ActivityError> {
        BackendActor::spawn(
            "rusteyes-x11-backend",
            move || Self::connect(lock_config),
            |mut backend, command_receiver, event_sender| {
                backend.run_actor(&command_receiver, &event_sender);
            },
        )
    }

    fn connect(lock_config: LockConfig) -> Result<Self, X11ActivityError> {
        Ok(Self {
            activity: X11Activity::connect()?,
            poller: ActivityPoller::new(POLL_INTERVAL),
            active_break: None,
            pending_break: None,
            lock_command: LockCommand::from(lock_config),
        })
    }

    fn run_actor(
        &mut self,
        command_receiver: &BackendCommandReceiver,
        event_sender: &BackendEventSender,
    ) {
        loop {
            while let Some(event) = self.poller.next_event() {
                if event_sender.send(event).is_err() {
                    return;
                }
            }

            match wait_for_command_or_timeout(command_receiver, self.next_sample_delay()) {
                BackendWait::Command(command) => self.handle_command(command),
                BackendWait::Timeout => {
                    if let Err(error) = self.sample_once() {
                        error!(%error, "backend error");
                        _ = event_sender.send(RuntimeEvent::Shutdown);
                        return;
                    }
                }
                BackendWait::Disconnected => return,
            }
        }
    }

    fn next_sample_delay(&self) -> Duration {
        if self.active_break.is_some() || self.pending_break.is_some() {
            OVERLAY_TICK_INTERVAL
        } else {
            self.poller.poll_interval()
        }
    }

    fn sample_once(&mut self) -> Result<(), X11ActivityError> {
        if self.active_break.is_some() {
            self.sample_overlay_once()
        } else if self.pending_break.is_some() {
            self.sample_pending_break_once()
        } else {
            self.sample_activity_once()
        }
    }

    fn sample_activity_once(&mut self) -> Result<(), X11ActivityError> {
        let sample = self.activity.sample()?;
        self.poller.queue_sample(sample);

        Ok(())
    }

    fn sample_overlay_once(&mut self) -> Result<(), X11ActivityError> {
        let sample = self.activity.sample()?;
        let break_elapsed = break_elapsed_for_sample(sample, OVERLAY_TICK_INTERVAL);
        trace!(
            idle_for = ?sample.idle_for(),
            state = ?sample.state_for(OVERLAY_TICK_INTERVAL),
            ?break_elapsed,
            break_time_advanced = !break_elapsed.is_zero(),
            "sampled X11 activity during break overlay"
        );
        let advance = match &mut self.active_break {
            Some(active_break) => active_break
                .advance(&self.activity.connection, break_elapsed)
                .map_err(|error| X11ActivityError::overlay(&error))?,
            None => BreakAdvance::default(),
        };

        self.poller
            .queue_event(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL));

        if advance.lock_after_break_requested {
            self.poller.queue_event(RuntimeEvent::LockAfterCurrentBreak);
        }

        if advance.finished {
            self.poller.queue_event(RuntimeEvent::BreakFinished);
        }

        Ok(())
    }

    fn sample_pending_break_once(&mut self) -> Result<(), X11ActivityError> {
        self.poller
            .queue_event(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL));

        let Some(mut pending_break) = self.pending_break.take() else {
            return Ok(());
        };
        pending_break.advance_wait(OVERLAY_TICK_INTERVAL);

        match self.show_break_overlay(&pending_break.scheduled_break) {
            Ok(overlay) => {
                self.active_break = Some(ActiveBreak::new(
                    overlay,
                    pending_break.scheduled_break.duration,
                ));
            }
            Err(error) if error.is_retryable_grab_failure() => {
                if pending_break.retry_timed_out() {
                    tracing::warn!(
                        %error,
                        waited = ?pending_break.waited,
                        "failed to start input-blocking X11 break overlay before retry timeout"
                    );
                    self.poller.queue_event(RuntimeEvent::BreakStartFailed);
                } else {
                    trace!(
                        %error,
                        waited = ?pending_break.waited,
                        "X11 break overlay input grab still unavailable; retrying"
                    );
                    self.pending_break = Some(pending_break);
                }
            }
            Err(error) => return Err(X11ActivityError::overlay(&error)),
        }

        Ok(())
    }

    fn start_break(&mut self, scheduled_break: &ScheduledBreak) {
        if let Err(error) = self.clear_break() {
            self.queue_backend_error(&error);
            return;
        }

        match self.show_break_overlay(scheduled_break) {
            Ok(overlay) => {
                self.active_break = Some(ActiveBreak::new(overlay, scheduled_break.duration));
            }
            Err(error) if error.is_retryable_grab_failure() => {
                tracing::warn!(
                    %error,
                    retry_timeout = ?BREAK_START_RETRY_TIMEOUT,
                    "X11 break overlay input grab unavailable; retrying"
                );
                self.pending_break = Some(PendingBreakStart::new(scheduled_break.clone()));
            }
            Err(error) => self.queue_backend_error(&X11ActivityError::overlay(&error)),
        }
    }

    fn show_break_overlay(
        &self,
        scheduled_break: &ScheduledBreak,
    ) -> Result<X11Overlay, X11OverlayError> {
        X11Overlay::show(
            &self.activity.connection,
            self.activity.screen,
            scheduled_break,
        )
    }

    fn clear_break(&mut self) -> Result<(), X11ActivityError> {
        self.pending_break = None;

        match self.active_break.take() {
            Some(active_break) => active_break
                .destroy(&self.activity.connection)
                .map_err(|error| X11ActivityError::overlay(&error)),
            None => Ok(()),
        }
    }

    fn finish_break(&mut self, lock_after: bool) {
        if self.active_break.is_some() {
            self.ring_break_finished_bell();
        }

        let lock_result = if lock_after && self.active_break.is_some() {
            match self.prepare_lock_handoff().and_then(|()| {
                start_lock_command(&self.lock_command)
                    .map_err(|error| X11ActivityError::lock_command(&error))
            }) {
                Ok(()) => {
                    thread::sleep(LOCK_HANDOFF_DELAY);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        let clear_result = self.clear_break();

        let mut first_error = lock_result.err();
        if let Err(error) = clear_result {
            if first_error.is_none() {
                first_error = Some(error);
            } else {
                error!(%error, "failed to clear break after lock handoff error");
            }
        }

        if let Some(error) = first_error {
            self.queue_backend_error(&error);
        }
    }

    fn ring_break_finished_bell(&self) {
        use x11rb::protocol::xproto::ConnectionExt as _;

        match self.activity.connection.bell(BREAK_FINISHED_BELL_PERCENT) {
            Ok(cookie) => {
                if let Err(error) = cookie.check() {
                    trace!(%error, "X11 bell request after break finished failed");
                }
            }
            Err(error) => {
                trace!(%error, "failed to send X11 bell request after break finished");
            }
        }
    }

    fn prepare_lock_handoff(&mut self) -> Result<(), X11ActivityError> {
        match &mut self.active_break {
            Some(active_break) => active_break
                .prepare_lock_handoff(&self.activity.connection)
                .map_err(|error| X11ActivityError::overlay(&error)),
            None => Ok(()),
        }
    }

    fn request_lock_after_current_break(&mut self) -> Result<(), X11ActivityError> {
        if let Some(active_break) = &mut self.active_break {
            active_break
                .request_lock_after_current_break(&self.activity.connection)
                .map_err(|error| X11ActivityError::overlay(&error))
        } else {
            if let Some(pending_break) = &mut self.pending_break {
                pending_break.request_lock_after_current_break();
            }
            Ok(())
        }
    }

    fn queue_backend_error(&mut self, error: &X11ActivityError) {
        error!(%error, "backend error");
        self.poller.queue_event(RuntimeEvent::Shutdown);
    }

    fn handle_command(&mut self, command: BackendCommand) {
        match command {
            BackendCommand::StartBreak(scheduled_break) => self.start_break(&scheduled_break),
            BackendCommand::FinishBreak { lock_after } => self.finish_break(lock_after),
            BackendCommand::RequestLockAfterCurrentBreak => {
                if let Err(error) = self.request_lock_after_current_break() {
                    self.queue_backend_error(&error);
                }
            }
            BackendCommand::ClearBreak => {
                if let Err(error) = self.clear_break() {
                    self.queue_backend_error(&error);
                }
            }
        }
    }
}

impl Drop for X11ActivityBackend {
    fn drop(&mut self) {
        if let Some(active_break) = self.active_break.take() {
            let _ = active_break.destroy(&self.activity.connection);
        }
    }
}

impl From<LockConfig> for LockCommand {
    fn from(lock_config: LockConfig) -> Self {
        let argv = lock_config
            .command
            .unwrap_or_else(|| DEFAULT_LOCK_COMMAND.into_iter().map(String::from).collect());
        Self::new(argv)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingBreakStart {
    scheduled_break: ScheduledBreak,
    waited: Duration,
}

impl PendingBreakStart {
    const fn new(scheduled_break: ScheduledBreak) -> Self {
        Self {
            scheduled_break,
            waited: Duration::ZERO,
        }
    }

    fn advance_wait(&mut self, elapsed: Duration) {
        self.waited = self.waited.saturating_add(elapsed);
    }

    fn retry_timed_out(&self) -> bool {
        self.waited >= BREAK_START_RETRY_TIMEOUT
    }

    fn request_lock_after_current_break(&mut self) {
        self.scheduled_break.autolock = true;
    }
}

struct ActiveBreak {
    overlay: X11Overlay,
    timer: BreakTimer,
}

impl ActiveBreak {
    const fn new(overlay: X11Overlay, duration: Duration) -> Self {
        Self {
            overlay,
            timer: BreakTimer::new(duration),
        }
    }

    fn advance(
        &mut self,
        connection: &RustConnection,
        elapsed: Duration,
    ) -> Result<BreakAdvance, X11OverlayError> {
        let lock_after_break_requested = self.overlay.handle_pending_events(connection)?;
        self.overlay.raise(connection)?;
        let finished = self.timer.advance(elapsed);
        self.overlay
            .update_remaining(connection, self.timer.remaining())?;

        Ok(BreakAdvance {
            finished,
            lock_after_break_requested,
        })
    }

    fn prepare_lock_handoff(&mut self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        self.overlay.raise(connection)?;
        self.overlay.release_input(connection)
    }

    fn request_lock_after_current_break(
        &mut self,
        connection: &RustConnection,
    ) -> Result<(), X11OverlayError> {
        self.overlay.request_lock_after_break(connection)
    }

    fn destroy(self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        self.overlay.destroy(connection)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BreakAdvance {
    finished: bool,
    lock_after_break_requested: bool,
}

struct X11Activity {
    connection: RustConnection,
    screen: X11Screen,
}

impl X11Activity {
    fn connect() -> Result<Self, X11ActivityError> {
        let (connection, screen_index) =
            x11rb::connect(None).map_err(|error| X11ActivityError::connect(error.to_string()))?;
        let screen = &connection.setup().roots[screen_index];
        let screen = X11Screen::new(
            screen.root,
            screen.root_depth,
            screen.width_in_pixels,
            screen.height_in_pixels,
            screen.black_pixel,
            screen.white_pixel,
        );

        connection
            .screensaver_query_version(
                SCREENSAVER_CLIENT_MAJOR_VERSION,
                SCREENSAVER_CLIENT_MINOR_VERSION,
            )
            .map_err(|error| X11ActivityError::query_version(error.to_string()))?
            .reply()
            .map_err(|error| X11ActivityError::query_version(error.to_string()))?;

        Ok(Self { connection, screen })
    }

    fn sample(&self) -> Result<ActivitySample, X11ActivityError> {
        let reply = self
            .connection
            .screensaver_query_info(self.screen.root())
            .map_err(|error| X11ActivityError::query_info(error.to_string()))?
            .reply()
            .map_err(|error| X11ActivityError::query_info(error.to_string()))?;

        Ok(ActivitySample::new(Duration::from_millis(u64::from(
            reply.ms_since_user_input,
        ))))
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct X11ActivityError {
    operation: &'static str,
    message: String,
}

impl X11ActivityError {
    fn connect(message: String) -> Self {
        Self {
            operation: "connect to X11",
            message,
        }
    }

    fn query_version(message: String) -> Self {
        Self {
            operation: "query XScreenSaver version",
            message,
        }
    }

    fn query_info(message: String) -> Self {
        Self {
            operation: "query X11 idle time",
            message,
        }
    }

    fn overlay(error: &X11OverlayError) -> Self {
        Self {
            operation: "manage X11 break overlay",
            message: error.to_string(),
        }
    }

    fn lock(message: String) -> Self {
        Self {
            operation: "request local lock",
            message,
        }
    }

    fn lock_command(error: &LockCommandError) -> Self {
        Self::lock(error.to_string())
    }
}

impl From<BackendActorSpawnError> for X11ActivityError {
    fn from(error: BackendActorSpawnError) -> Self {
        Self {
            operation: "start X11 backend actor",
            message: error.to_string(),
        }
    }
}

impl fmt::Display for X11ActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to {}: {}", self.operation, self.message)
    }
}

impl std::error::Error for X11ActivityError {}

#[cfg(test)]
mod tests;
