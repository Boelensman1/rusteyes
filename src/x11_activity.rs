use crate::backend::{Backend, BackendCommand, RuntimeEvent};
use crate::config::LockConfig;
use crate::scheduler::ScheduledBreak;
use crate::x11_overlay::{X11Overlay, X11OverlayError, X11Screen};
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing::{error, trace};
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt;
use x11rb::rust_connection::RustConnection;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const OVERLAY_TICK_INTERVAL: Duration = Duration::from_millis(500);
const SCREENSAVER_CLIENT_MAJOR_VERSION: u8 = 1;
const SCREENSAVER_CLIENT_MINOR_VERSION: u8 = 1;

#[allow(clippy::module_name_repetitions)]
pub(crate) struct X11ActivityBackend {
    activity: X11Activity,
    poller: ActivityPoller,
    active_break: Option<ActiveBreak>,
    lock_command: LockCommand,
}

impl X11ActivityBackend {
    pub(crate) fn connect(lock_config: LockConfig) -> Result<Self, X11ActivityError> {
        Ok(Self {
            activity: X11Activity::connect()?,
            poller: ActivityPoller::new(POLL_INTERVAL),
            active_break: None,
            lock_command: LockCommand::from(lock_config),
        })
    }

    fn poll_once(&mut self) -> Result<(), X11ActivityError> {
        if self.active_break.is_some() {
            self.poll_overlay_once()
        } else {
            self.poll_activity_once()
        }
    }

    fn poll_activity_once(&mut self) -> Result<(), X11ActivityError> {
        thread::sleep(self.poller.poll_interval());

        let sample = self.activity.sample()?;
        self.poller.queue_sample(sample);

        Ok(())
    }

    fn poll_overlay_once(&mut self) -> Result<(), X11ActivityError> {
        thread::sleep(OVERLAY_TICK_INTERVAL);

        let sample = self.activity.sample()?;
        let break_elapsed = break_elapsed_for_sample(sample, OVERLAY_TICK_INTERVAL);
        trace!(
            idle_for = ?sample.idle_for,
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

    fn start_break(&mut self, scheduled_break: &ScheduledBreak) {
        if let Err(error) = self.clear_break() {
            self.queue_backend_error(&error);
            return;
        }

        match X11Overlay::show(
            &self.activity.connection,
            self.activity.screen,
            scheduled_break,
        ) {
            Ok(overlay) => {
                self.active_break = Some(ActiveBreak::new(overlay, scheduled_break.duration));
            }
            Err(error) => self.queue_backend_error(&X11ActivityError::overlay(&error)),
        }
    }

    fn clear_break(&mut self) -> Result<(), X11ActivityError> {
        match self.active_break.take() {
            Some(active_break) => active_break
                .destroy(&self.activity.connection)
                .map_err(|error| X11ActivityError::overlay(&error)),
            None => Ok(()),
        }
    }

    fn request_lock(&mut self) {
        if let Err(error) = start_lock_command(&self.lock_command) {
            self.queue_backend_error(&error);
        }
    }

    fn queue_backend_error(&mut self, error: &X11ActivityError) {
        error!(%error, "backend error");
        self.poller.queue_event(RuntimeEvent::Shutdown);
    }
}

impl Backend for X11ActivityBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        loop {
            if let Some(event) = self.poller.next_event() {
                return event;
            }

            if let Err(error) = self.poll_once() {
                error!(%error, "backend error");
                return RuntimeEvent::Shutdown;
            }
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        match command {
            BackendCommand::StartBreak(scheduled_break) => self.start_break(&scheduled_break),
            BackendCommand::ClearBreak => {
                if let Err(error) = self.clear_break() {
                    self.queue_backend_error(&error);
                }
            }
            BackendCommand::RequestLock => self.request_lock(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LockCommand {
    argv: Vec<String>,
}

impl LockCommand {
    fn description(&self) -> String {
        if self.argv.is_empty() {
            String::from("<empty lock command>")
        } else {
            self.argv.join(" ")
        }
    }
}

impl From<LockConfig> for LockCommand {
    fn from(lock_config: LockConfig) -> Self {
        Self {
            argv: lock_config.command,
        }
    }
}

fn start_lock_command(lock_command: &LockCommand) -> Result<(), X11ActivityError> {
    let lock_command = lock_command.clone();
    let description = lock_command.description();
    let (startup_tx, startup_rx) = mpsc::channel();

    thread::Builder::new()
        .name(String::from("resteyes-lock-command"))
        .spawn(move || match spawn_lock_command(&lock_command) {
            Ok(spawned) => {
                let _ = startup_tx.send(Ok(()));
                supervise_lock_command(spawned);
            }
            Err(error) => {
                let _ = startup_tx.send(Err(error));
            }
        })
        .map_err(|error| {
            X11ActivityError::lock(format!(
                "failed to start supervisor for {description}: {error}"
            ))
        })?;

    startup_rx.recv().map_err(|error| {
        X11ActivityError::lock(format!(
            "failed to receive startup status for {description}: {error}"
        ))
    })?
}

fn spawn_lock_command(lock_command: &LockCommand) -> Result<SpawnedLockCommand, X11ActivityError> {
    let description = lock_command.description();
    let mut command = lock_process(lock_command)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        X11ActivityError::lock(format!("failed to start {description}: {error}"))
    })?;

    Ok(SpawnedLockCommand {
        description,
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        child,
    })
}

fn lock_process(lock_command: &LockCommand) -> Result<Command, X11ActivityError> {
    let Some((program, args)) = lock_command.argv.split_first() else {
        return Err(X11ActivityError::lock(String::from(
            "lock command must not be empty",
        )));
    };

    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

struct SpawnedLockCommand {
    description: String,
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

fn supervise_lock_command(spawned: SpawnedLockCommand) {
    let SpawnedLockCommand {
        description,
        mut child,
        stdout,
        stderr,
    } = spawned;

    thread::scope(|scope| {
        if let Some(stdout) = stdout {
            let description = description.clone();
            scope.spawn(move || trace_lock_stdout(&description, stdout));
        }

        if let Some(stderr) = stderr {
            let description = description.clone();
            scope.spawn(move || mirror_lock_stderr(&description, stderr));
        }

        match child.wait() {
            Ok(status) => {
                trace!(
                    command = %description,
                    %status,
                    success = status.success(),
                    "lock command exited"
                );
            }
            Err(error) => {
                error!(command = %description, %error, "failed to wait for lock command");
            }
        }
    });
}

fn trace_lock_stdout<R>(description: &str, output: R)
where
    R: Read,
{
    for line in BufReader::new(output).lines() {
        match line {
            Ok(line) => trace!(command = %description, %line, "lock command stdout"),
            Err(error) => {
                trace!(command = %description, %error, "failed to read lock command stdout");
                break;
            }
        }
    }
}

fn mirror_lock_stderr<R>(description: &str, output: R)
where
    R: Read,
{
    let mut stderr = io::stderr().lock();

    for line in BufReader::new(output).lines() {
        match line {
            Ok(line) => {
                let _ = writeln!(
                    stderr,
                    "resteyes: lock command stderr ({description}): {line}"
                );
                trace!(command = %description, %line, "lock command stderr");
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "resteyes: failed to read lock command stderr ({description}): {error}"
                );
                trace!(command = %description, %error, "failed to read lock command stderr");
                break;
            }
        }
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

    fn destroy(self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        self.overlay.destroy(connection)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BreakAdvance {
    finished: bool,
    lock_after_break_requested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BreakTimer {
    remaining: Duration,
    finished: bool,
}

impl BreakTimer {
    const fn new(duration: Duration) -> Self {
        Self {
            remaining: duration,
            finished: false,
        }
    }

    fn advance(&mut self, elapsed: Duration) -> bool {
        if self.finished {
            return false;
        }

        if elapsed >= self.remaining {
            self.remaining = Duration::ZERO;
            self.finished = true;
            true
        } else {
            self.remaining -= elapsed;
            false
        }
    }

    const fn remaining(self) -> Duration {
        self.remaining
    }
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
}

impl fmt::Display for X11ActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to {}: {}", self.operation, self.message)
    }
}

impl std::error::Error for X11ActivityError {}

#[derive(Debug)]
struct ActivityPoller {
    poll_interval: Duration,
    events: VecDeque<RuntimeEvent>,
}

impl ActivityPoller {
    fn new(poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            events: VecDeque::new(),
        }
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn queue_sample(&mut self, sample: ActivitySample) -> ActivityState {
        let state = sample.state_for(self.poll_interval);
        trace!(
            idle_for = ?sample.idle_for,
            ?state,
            poll_interval = ?self.poll_interval,
            "sampled X11 activity"
        );

        self.queue_event(RuntimeEvent::WallClockElapsed(self.poll_interval));

        if state == ActivityState::Active {
            self.queue_event(RuntimeEvent::ActiveTimeElapsed(self.poll_interval));
        }

        state
    }

    fn queue_event(&mut self, event: RuntimeEvent) {
        trace!(?event, "queued runtime event");
        self.events.push_back(event);
    }

    fn next_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop_front()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActivitySample {
    idle_for: Duration,
}

impl ActivitySample {
    const fn new(idle_for: Duration) -> Self {
        Self { idle_for }
    }

    fn state_for(self, poll_interval: Duration) -> ActivityState {
        if self.idle_for <= poll_interval {
            ActivityState::Active
        } else {
            ActivityState::Idle
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityState {
    Active,
    Idle,
}

fn break_elapsed_for_sample(sample: ActivitySample, poll_interval: Duration) -> Duration {
    match sample.state_for(poll_interval) {
        ActivityState::Active => Duration::ZERO,
        ActivityState::Idle => poll_interval,
    }
}

#[cfg(test)]
mod tests;
