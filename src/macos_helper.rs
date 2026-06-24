use crate::activity::{ActivityPoller, ActivitySample, BreakTimer, break_elapsed_for_sample};
use crate::backend::{
    BackendActor, BackendActorSpawnError, BackendCommand, BackendCommandReceiver,
    BackendEventSender, BackendWait, RuntimeEvent, wait_for_command_or_timeout,
};
use crate::config::LockConfig;
use crate::lock_command::{LockCommand, LockCommandError, start_lock_command};
use crate::scheduler::{BreakOrigin, ScheduledBreak};
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tracing::{trace, warn};

const PROTOCOL_VERSION: u16 = 8;
const HELPER_PATH_ENV: &str = "RUSTEYES_MACOS_HELPER";
const BREAK_DIAGNOSTICS_ENV: &str = "RUSTEYES_BREAK_DIAGNOSTICS";
const FORCE_CLEAR_PATH_ENV: &str = "RUSTEYES_FORCE_CLEAR_PATH";
const DEFAULT_FORCE_CLEAR_PATH: &str = "/tmp/rusteyes-force-clear";
const DEVELOPMENT_HELPER_PATH: &str = "helpers/macos-helper/.build/debug/rusteyes-macos-helper";
const BUNDLED_HELPER_PATH_FROM_EXE: &str = "../Resources/rusteyes-macos-helper";
const HELPER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const HELPER_SHUTDOWN_POLL: Duration = Duration::from_millis(20);
const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_secs(1);
const OVERLAY_TICK_INTERVAL: Duration = Duration::from_millis(500);
// Every helper response is preceded by a request the helper answers almost
// instantly, so this is a generous upper bound. Exceeding it means the helper is
// wedged; reads are bounded so the backend actor never blocks forever (which
// would keep `MacOSHelperBackend::drop` from tearing the helper down and leave
// the input-blocking event tap installed).
const HELPER_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[allow(clippy::module_name_repetitions)]
pub(crate) struct MacOSHelperBackend {
    child: Child,
    core: MacOSHelperCore<ChildStdin>,
    shutdown_sent: bool,
}

struct MacOSHelperCore<W> {
    session: HelperSession<W>,
    poller: ActivityPoller,
    active_break: Option<ActiveBreak>,
    lock: MacOSLock,
    diagnostics: BreakDiagnostics,
}

impl MacOSHelperBackend {
    pub(crate) fn spawn(lock_config: LockConfig) -> Result<BackendActor, MacOSHelperError> {
        BackendActor::spawn(
            "rusteyes-macos-backend",
            move || Self::connect(lock_config),
            |mut backend, command_receiver, event_sender| {
                backend.run_actor(&command_receiver, &event_sender);
            },
        )
    }

    fn connect(lock_config: LockConfig) -> Result<Self, MacOSHelperError> {
        let path = helper_path();
        let mut child = spawn_helper(&path)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MacOSHelperError::new("helper stdin was unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MacOSHelperError::new("helper stdout was unavailable"))?;

        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_mirror(&path, stderr);
        }

        let mut session = HelperSession::new(stdout, stdin);
        if let Err(error) = session.handshake() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = session.preflight_permissions() {
            shutdown_helper_after_startup_error(&mut child, &mut session);
            return Err(error);
        }

        Ok(Self {
            child,
            core: MacOSHelperCore::new(
                session,
                MacOSLock::from(lock_config),
                BreakDiagnostics::from_env(),
            ),
            shutdown_sent: false,
        })
    }

    fn run_actor(
        &mut self,
        command_receiver: &BackendCommandReceiver,
        event_sender: &BackendEventSender,
    ) {
        loop {
            while let Some(event) = self.core.next_event() {
                if event_sender.send(event).is_err() {
                    return;
                }
            }

            match wait_for_command_or_timeout(command_receiver, self.core.next_sample_delay()) {
                BackendWait::Command(command) => self.core.handle_command(command),
                BackendWait::Timeout => {
                    if let Err(error) = self.core.sample_once() {
                        warn!(%error, "failed to poll macOS activity");
                        _ = event_sender.send(RuntimeEvent::Shutdown);
                        return;
                    }
                }
                BackendWait::Disconnected => return,
            }
        }
    }
}

impl<W> MacOSHelperCore<W>
where
    W: Write,
{
    fn new(session: HelperSession<W>, lock: MacOSLock, diagnostics: BreakDiagnostics) -> Self {
        Self {
            session,
            poller: ActivityPoller::new(ACTIVITY_POLL_INTERVAL),
            active_break: None,
            lock,
            diagnostics,
        }
    }

    fn next_event(&mut self) -> Option<RuntimeEvent> {
        self.poller.next_event()
    }

    fn next_sample_delay(&self) -> Duration {
        if self.active_break.is_some() {
            OVERLAY_TICK_INTERVAL
        } else {
            self.poller.poll_interval()
        }
    }

    fn sample_once(&mut self) -> Result<(), MacOSHelperError> {
        if self.active_break.is_some() {
            self.sample_overlay_once()
        } else {
            self.sample_activity_once()
        }
    }

    fn sample_activity_once(&mut self) -> Result<(), MacOSHelperError> {
        self.diagnostics.discard_force_clear_trigger();
        let sample = self.session.poll_activity()?;
        self.poller.queue_sample(sample.activity);

        Ok(())
    }

    fn sample_overlay_once(&mut self) -> Result<(), MacOSHelperError> {
        if self.diagnostics.consume_force_clear_trigger() {
            return self.force_clear_active_break("shared force-clear file trigger");
        }

        let sample = self.session.poll_activity()?;
        self.diagnostics.log_helper_sample(&sample);
        if sample.force_exit_requested {
            let reason = sample
                .force_exit_reason
                .as_deref()
                .unwrap_or("helper force-exit request");
            self.finish_force_cleared_break(reason);
            return Ok(());
        }

        let break_elapsed = break_elapsed_for_sample(sample.activity, OVERLAY_TICK_INTERVAL);
        trace!(
            idle_for = ?sample.activity.idle_for(),
            state = ?sample.activity.state_for(OVERLAY_TICK_INTERVAL),
            ?break_elapsed,
            break_time_advanced = !break_elapsed.is_zero(),
            "sampled macOS activity during break overlay"
        );

        if let Some(update) = self
            .active_break
            .as_mut()
            .map(|active_break| active_break.apply_sample(&sample, break_elapsed))
        {
            self.diagnostics.log_overlay_sample(update, break_elapsed);
            if update.finished {
                self.finish_active_break(update.lock_after_break)?;
            } else {
                self.session
                    .update_break(update.remaining, update.lock_after_break)?;
            }
            queue_overlay_runtime_events(&mut self.poller, update);
        }

        Ok(())
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) {
        let duration = scheduled_break.duration;
        let lock_after_break = scheduled_break.autolock;
        let break_name = scheduled_break.name.clone();
        if self.send_backend_command(BackendCommand::StartBreak(scheduled_break)) {
            self.diagnostics.discard_force_clear_trigger();
            self.diagnostics
                .log_break_start(&break_name, duration, lock_after_break);
            self.active_break = Some(ActiveBreak::new(duration, lock_after_break));
        }
    }

    fn finish_break(&mut self, lock_after: bool) {
        if let Err(error) = self.finish_active_break(lock_after) {
            self.queue_backend_error(&error);
        }
    }

    fn finish_active_break(&mut self, lock_after: bool) -> Result<(), MacOSHelperError> {
        if self.active_break.take().is_none() {
            return Ok(());
        }

        let helper_lock_after = self.lock.helper_lock_after(lock_after);
        self.diagnostics
            .log_finish_break(lock_after, helper_lock_after);
        self.session.send_command(BackendCommand::FinishBreak {
            lock_after: helper_lock_after,
        })?;

        if lock_after {
            self.start_configured_lock_command()?;
        }

        Ok(())
    }

    fn start_configured_lock_command(&self) -> Result<(), MacOSHelperError> {
        let MacOSLock::Command(lock_command) = &self.lock else {
            return Ok(());
        };

        self.diagnostics
            .log_configured_lock_command(&lock_command.description());
        start_lock_command(lock_command).map_err(|error| MacOSHelperError::lock_command(&error))
    }

    fn clear_break(&mut self) {
        if self.send_backend_command(BackendCommand::ClearBreak) {
            self.active_break = None;
        }
    }

    fn request_lock_after_current_break(&mut self) {
        let Some((remaining, lock_after_break)) = self.active_break.as_mut().map(|active_break| {
            active_break.request_lock_after_break();
            (active_break.remaining(), active_break.lock_after_break())
        }) else {
            return;
        };

        if let Err(error) = self.session.update_break(remaining, lock_after_break) {
            self.queue_backend_error(&error);
        }
    }

    fn send_backend_command(&mut self, command: BackendCommand) -> bool {
        match self.session.send_command(command) {
            Ok(()) => true,
            Err(error) => {
                self.queue_backend_error(&error);
                false
            }
        }
    }

    fn queue_backend_error(&mut self, error: &MacOSHelperError) {
        warn!(%error, "macOS backend error");
        self.poller.queue_event(RuntimeEvent::Shutdown);
    }

    fn force_clear_active_break(&mut self, reason: &str) -> Result<(), MacOSHelperError> {
        if self.active_break.is_none() {
            return Ok(());
        }

        self.diagnostics.log_force_clear(reason);
        self.session.send_command(BackendCommand::ClearBreak)?;
        self.finish_force_cleared_break(reason);
        Ok(())
    }

    fn finish_force_cleared_break(&mut self, reason: &str) {
        if self.active_break.take().is_none() {
            return;
        }

        self.diagnostics.log_force_clear(reason);
        self.poller.queue_event(RuntimeEvent::BreakFinished);
    }

    fn handle_command(&mut self, command: BackendCommand) {
        match command {
            BackendCommand::StartBreak(scheduled_break) => self.start_break(scheduled_break),
            BackendCommand::ReplaceActiveBreak { message, remaining } => {
                self.replace_active_break(message, remaining);
            }
            BackendCommand::FinishBreak { lock_after } => self.finish_break(lock_after),
            BackendCommand::RequestLockAfterCurrentBreak => self.request_lock_after_current_break(),
            BackendCommand::ClearBreak => self.clear_break(),
        }
    }

    fn replace_active_break(&mut self, message: String, remaining: Duration) {
        let Some(lock_after_break) = self.active_break.as_mut().map(|active_break| {
            active_break.replace_remaining(remaining);
            active_break.lock_after_break()
        }) else {
            return;
        };

        if let Err(error) = self
            .session
            .replace_break(message, remaining, lock_after_break)
        {
            self.queue_backend_error(&error);
        }
    }
}

impl MacOSHelperBackend {
    fn shutdown_helper(&mut self) -> Result<(), MacOSHelperError> {
        shutdown_helper_process(
            &mut self.child,
            Some(&mut self.core.session),
            &mut self.shutdown_sent,
        )
    }
}

#[derive(Debug)]
struct BreakDiagnostics {
    enabled: bool,
    force_clear: Option<ForceClearTrigger>,
}

impl BreakDiagnostics {
    fn from_env() -> Self {
        let enabled = env_flag(BREAK_DIAGNOSTICS_ENV);
        if !enabled {
            return Self::disabled();
        }

        let path = env::var_os(FORCE_CLEAR_PATH_ENV)
            .map_or_else(|| PathBuf::from(DEFAULT_FORCE_CLEAR_PATH), PathBuf::from);
        let force_clear = match ForceClearTrigger::prepare(path) {
            Ok(trigger) => Some(trigger),
            Err(error) => {
                warn!(
                    %error,
                    "break diagnostics enabled but shared force-clear trigger is unavailable"
                );
                None
            }
        };

        let force_clear_path = force_clear
            .as_ref()
            .map(|trigger| trigger.path.display().to_string());
        warn!(?force_clear_path, "macOS break diagnostics enabled");

        Self {
            enabled,
            force_clear,
        }
    }

    const fn disabled() -> Self {
        Self {
            enabled: false,
            force_clear: None,
        }
    }

    fn discard_force_clear_trigger(&mut self) {
        if !self.enabled {
            return;
        }

        let Some(trigger) = &mut self.force_clear else {
            return;
        };
        if let Err(error) = trigger.discard_pending_write() {
            warn!(%error, path = %trigger.path.display(), "disabled force-clear trigger");
            self.force_clear = None;
        }
    }

    fn consume_force_clear_trigger(&mut self) -> bool {
        if !self.enabled {
            return false;
        }

        let Some(trigger) = &mut self.force_clear else {
            return false;
        };
        match trigger.consume_if_changed() {
            Ok(changed) => changed,
            Err(error) => {
                warn!(%error, path = %trigger.path.display(), "disabled force-clear trigger");
                self.force_clear = None;
                false
            }
        }
    }

    fn log_break_start(&self, name: &str, duration: Duration, lock_after: bool) {
        if self.enabled {
            warn!(
                break_name = name,
                ?duration,
                lock_after,
                "diagnostic macOS break started"
            );
        }
    }

    fn log_overlay_sample(&self, update: ActiveBreakUpdate, elapsed: Duration) {
        if self.enabled && update.remaining <= Duration::from_secs(5) {
            warn!(
                remaining = ?update.remaining,
                lock_after = update.lock_after_break,
                lock_requested = update.lock_after_break_requested,
                finished = update.finished,
                ?elapsed,
                "diagnostic macOS overlay sample near finish"
            );
        }
    }

    fn log_helper_sample(&self, sample: &HelperActivitySample) {
        if self.enabled {
            warn!(
                idle_for = ?sample.activity.idle_for(),
                lock_requested = sample.lock_after_break_requested,
                force_exit_requested = sample.force_exit_requested,
                force_exit_reason = ?sample.force_exit_reason,
                overlay_state = ?sample.overlay_state,
                "diagnostic received macOS helper activity sample"
            );
        }
    }

    fn log_finish_break(&self, lock_after: bool, helper_lock_after: bool) {
        if self.enabled {
            warn!(
                lock_after,
                helper_lock_after, "diagnostic macOS backend sending finishBreak"
            );
        }
    }

    fn log_configured_lock_command(&self, command: &str) {
        if self.enabled {
            warn!(%command, "diagnostic macOS backend starting configured lock command");
        }
    }

    fn log_force_clear(&self, reason: &str) {
        if self.enabled {
            warn!(reason, "diagnostic macOS force-clearing active break");
        }
    }
}

#[derive(Debug)]
struct ForceClearTrigger {
    path: PathBuf,
    observed: ForceClearSnapshot,
}

impl ForceClearTrigger {
    fn prepare(path: PathBuf) -> Result<Self, MacOSHelperError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(true)
            .write(true)
            .mode(0o666)
            .open(&path)
            .map_err(|error| {
                MacOSHelperError::new(format!("failed to open {}: {error}", path.display()))
            })?;
        let metadata = file.metadata().map_err(|error| {
            MacOSHelperError::new(format!("failed to inspect {}: {error}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(MacOSHelperError::new(format!(
                "force-clear path is not a regular file: {}",
                path.display()
            )));
        }

        file.set_permissions(fs::Permissions::from_mode(0o666))
            .map_err(|error| {
                MacOSHelperError::new(format!(
                    "failed to make {} world-writable: {error}",
                    path.display()
                ))
            })?;
        file.set_len(0).map_err(|error| {
            MacOSHelperError::new(format!("failed to reset {}: {error}", path.display()))
        })?;
        let metadata = file.metadata().map_err(|error| {
            MacOSHelperError::new(format!(
                "failed to inspect reset {}: {error}",
                path.display()
            ))
        })?;

        Ok(Self {
            path,
            observed: ForceClearSnapshot::from_metadata(&metadata),
        })
    }

    fn discard_pending_write(&mut self) -> Result<(), MacOSHelperError> {
        if self.changed()? {
            self.reset_observed()?;
        }
        Ok(())
    }

    fn consume_if_changed(&mut self) -> Result<bool, MacOSHelperError> {
        if !self.changed()? {
            return Ok(false);
        }

        self.reset_observed()?;
        Ok(true)
    }

    fn changed(&self) -> Result<bool, MacOSHelperError> {
        let metadata = fs::metadata(&self.path).map_err(|error| {
            MacOSHelperError::new(format!(
                "failed to inspect {}: {error}",
                self.path.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(MacOSHelperError::new(format!(
                "force-clear path is no longer a regular file: {}",
                self.path.display()
            )));
        }

        Ok(ForceClearSnapshot::from_metadata(&metadata) != self.observed)
    }

    fn reset_observed(&mut self) -> Result<(), MacOSHelperError> {
        let file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|error| {
                MacOSHelperError::new(format!(
                    "failed to open {} for reset: {error}",
                    self.path.display()
                ))
            })?;
        file.set_len(0).map_err(|error| {
            MacOSHelperError::new(format!("failed to reset {}: {error}", self.path.display()))
        })?;
        self.observed = ForceClearSnapshot::from_metadata(&file.metadata().map_err(|error| {
            MacOSHelperError::new(format!(
                "failed to inspect reset {}: {error}",
                self.path.display()
            ))
        })?);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForceClearSnapshot {
    len: u64,
    modified: Option<SystemTime>,
}

impl ForceClearSnapshot {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MacOSLock {
    PlatformDefault,
    Command(LockCommand),
}

impl MacOSLock {
    const fn helper_lock_after(&self, requested: bool) -> bool {
        requested && matches!(self, Self::PlatformDefault)
    }
}

impl From<LockConfig> for MacOSLock {
    fn from(lock_config: LockConfig) -> Self {
        match lock_config.command {
            Some(command) => Self::Command(LockCommand::new(command)),
            None => Self::PlatformDefault,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveBreak {
    timer: BreakTimer,
    lock_after_break: bool,
}

impl ActiveBreak {
    const fn new(duration: Duration, lock_after_break: bool) -> Self {
        Self {
            timer: BreakTimer::new(duration),
            lock_after_break,
        }
    }

    fn apply_sample(
        &mut self,
        sample: &HelperActivitySample,
        elapsed: Duration,
    ) -> ActiveBreakUpdate {
        if sample.lock_after_break_requested {
            self.request_lock_after_break();
        }

        let finished = self.advance(elapsed);

        ActiveBreakUpdate {
            remaining: self.remaining(),
            lock_after_break: self.lock_after_break(),
            lock_after_break_requested: sample.lock_after_break_requested,
            finished,
        }
    }

    fn advance(&mut self, elapsed: Duration) -> bool {
        self.timer.advance(elapsed)
    }

    fn request_lock_after_break(&mut self) {
        self.lock_after_break = true;
    }

    fn replace_remaining(&mut self, remaining: Duration) {
        self.timer = BreakTimer::new(remaining);
    }

    const fn remaining(self) -> Duration {
        self.timer.remaining()
    }

    const fn lock_after_break(self) -> bool {
        self.lock_after_break
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveBreakUpdate {
    remaining: Duration,
    lock_after_break: bool,
    lock_after_break_requested: bool,
    finished: bool,
}

fn queue_overlay_runtime_events(poller: &mut ActivityPoller, update: ActiveBreakUpdate) {
    poller.queue_event(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL));

    if update.lock_after_break_requested {
        poller.queue_event(RuntimeEvent::LockAfterCurrentBreak);
    }

    if update.finished {
        poller.queue_event(RuntimeEvent::BreakFinished);
    }
}

impl Drop for MacOSHelperBackend {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_helper() {
            warn!(%error, "failed to shut down macOS helper cleanly");
        }
    }
}

#[derive(Debug)]
pub(crate) struct MacOSHelperError {
    message: String,
}

impl MacOSHelperError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn lock_command(error: &LockCommandError) -> Self {
        Self::new(format!("failed to request local lock: {error}"))
    }
}

impl From<BackendActorSpawnError> for MacOSHelperError {
    fn from(error: BackendActorSpawnError) -> Self {
        Self::new(error.to_string())
    }
}

impl fmt::Display for MacOSHelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "macOS helper error: {}", self.message)
    }
}

impl std::error::Error for MacOSHelperError {}

/// One line read from the helper, or a description of the read failure. The
/// reader thread drops its sender on EOF, which surfaces as a disconnect.
type HelperLine = Result<String, String>;

struct HelperSession<W> {
    lines: flume::Receiver<HelperLine>,
    read_timeout: Duration,
    writer: W,
}

impl<W> HelperSession<W>
where
    W: Write,
{
    fn new<R>(reader: R, writer: W) -> Self
    where
        R: Read + Send + 'static,
    {
        Self::with_read_timeout(reader, writer, HELPER_READ_TIMEOUT)
    }

    fn with_read_timeout<R>(reader: R, writer: W, read_timeout: Duration) -> Self
    where
        R: Read + Send + 'static,
    {
        let (sender, lines) = flume::unbounded();
        // Plain `thread::spawn` returns the handle directly (no fallible
        // `Builder`); the reader thread is detached and ends when the pipe
        // closes, so there is nothing to join or recover from.
        thread::spawn(move || read_helper_lines(reader, &sender));

        Self {
            lines,
            read_timeout,
            writer,
        }
    }

    fn handshake(&mut self) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::Hello {
            version: PROTOCOL_VERSION,
        })?;

        let version = self.receive_expected(
            "helper ready message during handshake",
            "handshake",
            |message| match message {
                HelperMessage::Ready { version } => Ok(version),
                message => Err(message),
            },
        )?;

        if version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(MacOSHelperError::new(format!(
                "helper protocol version {version} is incompatible with daemon protocol version {PROTOCOL_VERSION}"
            )))
        }
    }

    fn send_command(&mut self, command: BackendCommand) -> Result<(), MacOSHelperError> {
        let expected_command = HelperCommand::from_backend_command(&command);
        self.send_helper_command(&DaemonMessage::from(command), expected_command)
    }

    fn update_break(
        &mut self,
        remaining: Duration,
        lock_after: bool,
    ) -> Result<(), MacOSHelperError> {
        self.send_helper_command(
            &DaemonMessage::UpdateBreak {
                remaining_ms: duration_millis(remaining),
                lock_after,
                message: None,
            },
            HelperCommand::Update,
        )
    }

    fn replace_break(
        &mut self,
        message: String,
        remaining: Duration,
        lock_after: bool,
    ) -> Result<(), MacOSHelperError> {
        self.send_helper_command(
            &DaemonMessage::UpdateBreak {
                remaining_ms: duration_millis(remaining),
                lock_after,
                message: Some(message),
            },
            HelperCommand::Update,
        )
    }

    fn send_helper_command(
        &mut self,
        message: &DaemonMessage,
        expected_command: HelperCommand,
    ) -> Result<(), MacOSHelperError> {
        if env_flag(BREAK_DIAGNOSTICS_ENV) {
            warn!(
                command = expected_command.as_str(),
                "diagnostic sending macOS helper command"
            );
        }
        self.send(message)?;

        let error_context = format!("{} command", expected_command.as_str());
        let command =
            self.receive_expected("helper command completion", &error_context, |message| {
                match message {
                    HelperMessage::CommandComplete { command } => Ok(command),
                    message => Err(message),
                }
            })?;

        if command == expected_command {
            if env_flag(BREAK_DIAGNOSTICS_ENV) {
                warn!(
                    command = command.as_str(),
                    "diagnostic received macOS helper command completion"
                );
            }
            Ok(())
        } else {
            Err(MacOSHelperError::new(format!(
                "expected helper command completion for {}, got {}",
                expected_command.as_str(),
                command.as_str()
            )))
        }
    }

    fn preflight_permissions(&mut self) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::PreflightPermissions)?;

        self.receive_expected(
            "helper permission preflight result",
            "permission preflight",
            |message| match message {
                HelperMessage::PreflightResult {
                    accessibility_trusted,
                    input_monitoring_trusted,
                } => Ok(PermissionPreflight {
                    accessibility_trusted,
                    input_monitoring_trusted,
                }),
                message => Err(message),
            },
        )?
        .ensure_trusted()
    }

    fn poll_activity(&mut self) -> Result<HelperActivitySample, MacOSHelperError> {
        if env_flag(BREAK_DIAGNOSTICS_ENV) {
            warn!("diagnostic sending macOS helper pollActivity");
        }
        self.send(&DaemonMessage::PollActivity)?;

        self.receive_expected(
            "helper activity sample",
            "activity",
            |message| match message {
                HelperMessage::ActivitySample {
                    idle_ms,
                    lock_after_break_requested,
                    force_exit_requested,
                    force_exit_reason,
                    overlay_state,
                } => Ok(HelperActivitySample {
                    activity: ActivitySample::new(Duration::from_millis(idle_ms)),
                    lock_after_break_requested,
                    force_exit_requested,
                    force_exit_reason,
                    overlay_state,
                }),
                message => Err(message),
            },
        )
    }

    fn send_shutdown(&mut self) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::Shutdown)
    }

    fn receive_shutdown_complete(&mut self) -> Result<(), MacOSHelperError> {
        self.receive_expected(
            "helper shutdown completion",
            "shutdown",
            |message| match message {
                HelperMessage::ShutdownComplete => Ok(()),
                message => Err(message),
            },
        )
    }

    fn shutdown(&mut self) -> Result<(), MacOSHelperError> {
        self.send_shutdown()?;
        self.receive_shutdown_complete()
    }

    fn send(&mut self, message: &DaemonMessage) -> Result<(), MacOSHelperError> {
        serde_json::to_writer(&mut self.writer, message).map_err(|error| {
            MacOSHelperError::new(format!("failed to encode helper message: {error}"))
        })?;
        self.writer.write_all(b"\n").map_err(|error| {
            MacOSHelperError::new(format!("failed to write helper message: {error}"))
        })?;
        self.writer.flush().map_err(|error| {
            MacOSHelperError::new(format!("failed to flush helper message: {error}"))
        })
    }

    fn receive(&mut self) -> Result<HelperMessage, MacOSHelperError> {
        let line = match self.lines.recv_timeout(self.read_timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                return Err(MacOSHelperError::new(format!(
                    "failed to read helper message: {error}"
                )));
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                return Err(MacOSHelperError::new(format!(
                    "helper did not respond within {} ms; treating it as unresponsive",
                    duration_millis(self.read_timeout)
                )));
            }
            Err(flume::RecvTimeoutError::Disconnected) => {
                return Err(MacOSHelperError::new(
                    "helper closed protocol output before sending a message",
                ));
            }
        };

        serde_json::from_str(line.trim_end()).map_err(|error| {
            MacOSHelperError::new(format!("failed to decode helper message: {error}"))
        })
    }

    fn receive_expected<T>(
        &mut self,
        expected: &str,
        helper_error_context: &str,
        decode: impl FnOnce(HelperMessage) -> Result<T, HelperMessage>,
    ) -> Result<T, MacOSHelperError> {
        let message = self.receive()?;
        match message {
            HelperMessage::Error { message } => Err(MacOSHelperError::new(format!(
                "helper reported {helper_error_context} error: {message}"
            ))),
            message => decode(message).map_err(|message| {
                MacOSHelperError::new(format!(
                    "expected {expected}, got {}",
                    message.message_type()
                ))
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DaemonMessage {
    Hello {
        version: u16,
    },
    PreflightPermissions,
    StartBreak {
        #[serde(rename = "break")]
        break_info: WireBreak,
    },
    FinishBreak {
        #[serde(rename = "lockAfter")]
        lock_after: bool,
    },
    UpdateBreak {
        #[serde(rename = "remainingMs")]
        remaining_ms: u64,
        #[serde(rename = "lockAfter")]
        lock_after: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    ClearBreak,
    PollActivity,
    Shutdown,
}

impl From<BackendCommand> for DaemonMessage {
    fn from(command: BackendCommand) -> Self {
        match command {
            BackendCommand::StartBreak(scheduled_break) => Self::StartBreak {
                break_info: WireBreak::from(scheduled_break),
            },
            BackendCommand::FinishBreak { lock_after } => Self::FinishBreak { lock_after },
            BackendCommand::RequestLockAfterCurrentBreak => {
                unreachable!(
                    "lock-after-current-break updates are framed with HelperSession::update_break"
                )
            }
            BackendCommand::ReplaceActiveBreak { .. } => {
                unreachable!("active-break replacement is framed with HelperSession::replace_break")
            }
            BackendCommand::ClearBreak => Self::ClearBreak,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBreak {
    name: String,
    origin: WireBreakOrigin,
    duration_ms: u64,
    message: String,
    autolock: bool,
}

impl From<ScheduledBreak> for WireBreak {
    fn from(scheduled_break: ScheduledBreak) -> Self {
        Self {
            name: scheduled_break.name.clone(),
            origin: WireBreakOrigin::from(scheduled_break.origin),
            duration_ms: duration_millis(scheduled_break.duration),
            message: scheduled_break.message.clone(),
            autolock: scheduled_break.autolock,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum WireBreakOrigin {
    Scheduled { slot: usize },
    Manual,
}

impl From<BreakOrigin> for WireBreakOrigin {
    fn from(origin: BreakOrigin) -> Self {
        match origin {
            BreakOrigin::Scheduled { slot } => Self::Scheduled { slot },
            BreakOrigin::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum HelperCommand {
    #[serde(rename = "startBreak")]
    Start,
    #[serde(rename = "finishBreak")]
    Finish,
    #[serde(rename = "updateBreak")]
    Update,
    #[serde(rename = "clearBreak")]
    Clear,
}

impl HelperCommand {
    fn from_backend_command(command: &BackendCommand) -> Self {
        match command {
            BackendCommand::StartBreak(_) => Self::Start,
            BackendCommand::FinishBreak { .. } => Self::Finish,
            BackendCommand::RequestLockAfterCurrentBreak => {
                unreachable!(
                    "lock-after-current-break updates are framed with HelperSession::update_break"
                )
            }
            BackendCommand::ReplaceActiveBreak { .. } => {
                unreachable!("active-break replacement is framed with HelperSession::replace_break")
            }
            BackendCommand::ClearBreak => Self::Clear,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "startBreak",
            Self::Finish => "finishBreak",
            Self::Update => "updateBreak",
            Self::Clear => "clearBreak",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelperActivitySample {
    activity: ActivitySample,
    lock_after_break_requested: bool,
    force_exit_requested: bool,
    force_exit_reason: Option<String>,
    overlay_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum HelperMessage {
    Ready {
        version: u16,
    },
    ActivitySample {
        #[serde(rename = "idleMs")]
        idle_ms: u64,
        #[serde(rename = "lockAfterBreakRequested")]
        lock_after_break_requested: bool,
        #[serde(rename = "forceExitRequested", default)]
        force_exit_requested: bool,
        #[serde(rename = "forceExitReason", default)]
        force_exit_reason: Option<String>,
        #[serde(rename = "overlayState", default)]
        overlay_state: Option<String>,
    },
    PreflightResult {
        #[serde(rename = "accessibilityTrusted")]
        accessibility_trusted: bool,
        #[serde(rename = "inputMonitoringTrusted")]
        input_monitoring_trusted: bool,
    },
    CommandComplete {
        command: HelperCommand,
    },
    ShutdownComplete,
    Error {
        message: String,
    },
}

impl HelperMessage {
    const fn message_type(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::ActivitySample { .. } => "activitySample",
            Self::PreflightResult { .. } => "preflightResult",
            Self::CommandComplete { .. } => "commandComplete",
            Self::ShutdownComplete => "shutdownComplete",
            Self::Error { .. } => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PermissionPreflight {
    accessibility_trusted: bool,
    input_monitoring_trusted: bool,
}

impl PermissionPreflight {
    fn ensure_trusted(self) -> Result<(), MacOSHelperError> {
        let mut missing = Vec::new();

        if !self.accessibility_trusted {
            missing.push("Accessibility");
        }
        if !self.input_monitoring_trusted {
            missing.push("Input Monitoring");
        }

        if missing.is_empty() {
            return Ok(());
        }

        let plural = if missing.len() == 1 { "" } else { "s" };
        let missing_permissions = permission_list(&missing);
        Err(MacOSHelperError::new(format!(
            "missing macOS privacy permission{plural}: {missing_permissions}. Open System Settings > Privacy & Security and grant the listed permission{plural} to RustEyes, then restart RustEyes. Development builds may appear as rusteyes-macos-helper."
        )))
    }
}

fn permission_list(permissions: &[&str]) -> String {
    match permissions {
        [] => String::new(),
        [permission] => (*permission).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => permissions.join(", "),
    }
}

/// Reads newline-delimited helper output on a dedicated thread so the backend
/// actor can bound its waits with [`flume::Receiver::recv_timeout`] instead of
/// blocking forever on a wedged helper. EOF drops the sender, which the actor
/// observes as a disconnect.
fn read_helper_lines<R>(reader: R, sender: &flume::Sender<HelperLine>)
where
    R: Read,
{
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if sender.send(Ok(line)).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                break;
            }
        }
    }
}

fn spawn_helper(path: &Path) -> Result<Child, MacOSHelperError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MacOSHelperError::new(format!(
            "failed to find helper at {}: {error}",
            path.display()
        ))
    })?;

    if !metadata.is_file() {
        return Err(MacOSHelperError::new(format!(
            "helper path is not a file: {}",
            path.display()
        )));
    }

    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().map_err(|error| {
        MacOSHelperError::new(format!(
            "failed to start helper at {}: {error}",
            path.display()
        ))
    })
}

fn shutdown_helper_after_startup_error(child: &mut Child, session: &mut HelperSession<ChildStdin>) {
    let mut shutdown_sent = false;
    if let Err(error) = shutdown_helper_process(child, Some(session), &mut shutdown_sent) {
        warn!(%error, "failed to shut down macOS helper after startup error");
    }
}

fn shutdown_helper_process(
    child: &mut Child,
    session: Option<&mut HelperSession<ChildStdin>>,
    shutdown_sent: &mut bool,
) -> Result<(), MacOSHelperError> {
    let mut first_error = None;

    if !*shutdown_sent {
        match session {
            Some(session) => match session.shutdown() {
                Ok(()) => {
                    *shutdown_sent = true;
                }
                Err(error) => remember_first_error(&mut first_error, error),
            },
            None => remember_first_error(
                &mut first_error,
                MacOSHelperError::new("helper shutdown requested without a protocol session"),
            ),
        }
    }

    if let Err(error) = wait_for_helper_exit_or_kill(child) {
        remember_first_error(&mut first_error, error);
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn wait_for_helper_exit_or_kill(child: &mut Child) -> Result<(), MacOSHelperError> {
    let started = Instant::now();
    while started.elapsed() < HELPER_SHUTDOWN_TIMEOUT {
        match child.try_wait() {
            Ok(Some(status)) => {
                trace!(%status, "macOS helper exited");
                return Ok(());
            }
            Ok(None) => thread::sleep(HELPER_SHUTDOWN_POLL),
            Err(error) => {
                return Err(MacOSHelperError::new(format!(
                    "failed to poll helper exit: {error}"
                )));
            }
        }
    }

    let mut first_error = Some(MacOSHelperError::new(format!(
        "helper did not exit within {} ms after shutdown",
        duration_millis(HELPER_SHUTDOWN_TIMEOUT)
    )));

    if let Err(error) = child.kill() {
        remember_first_error(
            &mut first_error,
            MacOSHelperError::new(format!("failed to kill unresponsive helper: {error}")),
        );
    }
    if let Err(error) = child.wait() {
        remember_first_error(
            &mut first_error,
            MacOSHelperError::new(format!("failed to reap helper after timeout: {error}")),
        );
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn spawn_stderr_mirror(path: &Path, stderr: ChildStderr) {
    let description = path.display().to_string();
    if let Err(error) = thread::Builder::new()
        .name(String::from("rusteyes-macos-helper-stderr"))
        .spawn(move || mirror_helper_stderr(&description, stderr))
    {
        warn!(%error, helper = %path.display(), "failed to start helper stderr mirror");
    }
}

fn mirror_helper_stderr<R>(description: &str, output: R)
where
    R: Read,
{
    let mut stderr = io::stderr().lock();

    for line in BufReader::new(output).lines() {
        match line {
            Ok(line) => {
                let _ = writeln!(
                    stderr,
                    "rusteyes: macOS helper stderr ({description}): {line}"
                );
                trace!(helper = %description, %line, "macOS helper stderr");
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "rusteyes: failed to read macOS helper stderr ({description}): {error}"
                );
                trace!(helper = %description, %error, "failed to read macOS helper stderr");
                break;
            }
        }
    }
}

fn helper_path() -> PathBuf {
    if let Some(path) = env::var_os(HELPER_PATH_ENV) {
        return PathBuf::from(path);
    }

    if let Some(path) = bundled_helper_path()
        && path.is_file()
    {
        return path;
    }

    development_helper_path()
}

fn bundled_helper_path() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let executable_dir = executable.parent()?;
    Some(executable_dir.join(BUNDLED_HELPER_PATH_FROM_EXE))
}

fn development_helper_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(DEVELOPMENT_HELPER_PATH)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn remember_first_error(first_error: &mut Option<MacOSHelperError>, error: MacOSHelperError) {
    if first_error.is_none() {
        *first_error = Some(error);
    } else {
        warn!(%error, "additional macOS helper shutdown error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::time::UNIX_EPOCH;

    #[test]
    fn handshake_writes_hello_and_accepts_ready() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"ready","version":8}"#.to_vec());
        let mut output = Vec::new();

        {
            let mut session = HelperSession::new(input, &mut output);
            session.handshake()?;
        }

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::Hello {
                version: PROTOCOL_VERSION,
            }]
        );
        Ok(())
    }

    #[test]
    fn preflight_permissions_writes_request_and_accepts_trusted_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"preflightResult","accessibilityTrusted":true,"inputMonitoringTrusted":true}"#
                .to_vec(),
        );
        let mut output = Vec::new();

        {
            let mut session = HelperSession::new(input, &mut output);
            session.preflight_permissions()?;
        }

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::PreflightPermissions]
        );
        Ok(())
    }

    #[test]
    fn preflight_permissions_errors_when_accessibility_is_missing() {
        let result = PermissionPreflight {
            accessibility_trusted: false,
            input_monitoring_trusted: true,
        }
        .ensure_trusted();
        let Err(error) = result else {
            panic!("missing Accessibility must fail preflight");
        };

        let message = error.to_string();
        assert!(message.contains("Accessibility"));
        assert!(!message.contains("Input Monitoring."));
        assert!(message.contains("System Settings > Privacy & Security"));
        assert!(message.contains("listed permission"));
        assert!(message.contains("rusteyes-macos-helper"));
    }

    #[test]
    fn preflight_permissions_errors_when_input_monitoring_is_missing() {
        let result = PermissionPreflight {
            accessibility_trusted: true,
            input_monitoring_trusted: false,
        }
        .ensure_trusted();
        let Err(error) = result else {
            panic!("missing Input Monitoring must fail preflight");
        };

        let message = error.to_string();
        assert!(message.contains("Input Monitoring"));
        assert!(!message.contains("Accessibility."));
        assert!(message.contains("System Settings > Privacy & Security"));
        assert!(message.contains("listed permission"));
        assert!(message.contains("rusteyes-macos-helper"));
    }

    #[test]
    fn preflight_permissions_errors_when_both_permissions_are_missing() {
        let result = PermissionPreflight {
            accessibility_trusted: false,
            input_monitoring_trusted: false,
        }
        .ensure_trusted();
        let Err(error) = result else {
            panic!("missing permissions must fail preflight");
        };

        let message = error.to_string();
        assert!(message.contains("Accessibility and Input Monitoring"));
        assert!(message.contains("missing macOS privacy permissions"));
        assert!(message.contains("listed permissions"));
        assert!(message.contains("restart RustEyes"));
    }

    #[test]
    fn handshake_rejects_incompatible_version() {
        let input = Cursor::new(br#"{"type":"ready","version":9}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        let Err(error) = session.handshake() else {
            panic!("handshake must fail");
        };

        assert!(
            error.to_string().contains("incompatible"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn poll_activity_writes_request_and_decodes_sample() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":750,"lockAfterBreakRequested":true}"#.to_vec(),
        );
        let mut output = Vec::new();
        let sample = {
            let mut session = HelperSession::new(input, &mut output);
            session.poll_activity()?
        };

        assert_eq!(sample.activity.idle_for(), Duration::from_millis(750));
        assert!(sample.lock_after_break_requested);
        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::PollActivity]);
        Ok(())
    }

    #[test]
    fn start_break_command_uses_millisecond_wire_duration() -> Result<(), Box<dyn std::error::Error>>
    {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"startBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.send_command(BackendCommand::StartBreak(ScheduledBreak {
            name: String::from("short"),
            origin: BreakOrigin::Scheduled { slot: 3 },
            duration: Duration::from_millis(1_500),
            message: String::from("Rest your eyes"),
            autolock: true,
        }))?;

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::StartBreak {
                break_info: WireBreak {
                    name: String::from("short"),
                    origin: WireBreakOrigin::Scheduled { slot: 3 },
                    duration_ms: 1_500,
                    message: String::from("Rest your eyes"),
                    autolock: true,
                },
            }]
        );
        Ok(())
    }

    #[test]
    fn finish_break_command_is_framed() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"finishBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.send_command(BackendCommand::FinishBreak { lock_after: true })?;

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::FinishBreak { lock_after: true }]
        );
        let messages = daemon_json_values(&output)?;
        assert_eq!(messages[0]["lockAfter"], true);
        assert!(messages[0].get("lock_after").is_none());
        Ok(())
    }

    #[test]
    fn update_break_command_is_framed() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"updateBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.update_break(Duration::from_millis(2_500), true)?;

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::UpdateBreak {
                remaining_ms: 2_500,
                lock_after: true,
                message: None,
            }]
        );
        let messages = daemon_json_values(&output)?;
        assert_eq!(messages[0]["remainingMs"], 2_500);
        assert_eq!(messages[0]["lockAfter"], true);
        assert!(messages[0].get("lock_after").is_none());
        assert!(messages[0].get("message").is_none());
        Ok(())
    }

    #[test]
    fn replace_break_command_carries_message() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"updateBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.replace_break(
            String::from("Look away"),
            Duration::from_millis(2_500),
            false,
        )?;

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::UpdateBreak {
                remaining_ms: 2_500,
                lock_after: false,
                message: Some(String::from("Look away")),
            }]
        );
        let messages = daemon_json_values(&output)?;
        assert_eq!(messages[0]["message"], "Look away");
        Ok(())
    }

    #[test]
    fn default_macos_lock_requests_helper_lock() {
        let lock = MacOSLock::from(LockConfig { command: None });

        assert_eq!(lock, MacOSLock::PlatformDefault);
        assert!(lock.helper_lock_after(true));
        assert!(!lock.helper_lock_after(false));
    }

    #[test]
    fn explicit_macos_lock_command_uses_rust_command_runner() {
        let lock = MacOSLock::from(LockConfig {
            command: Some(vec![String::from("locker"), String::from("--now")]),
        });

        assert_eq!(
            lock,
            MacOSLock::Command(LockCommand::new(vec![
                String::from("locker"),
                String::from("--now")
            ]))
        );
        assert!(!lock.helper_lock_after(true));
        assert!(!lock.helper_lock_after(false));
    }

    #[test]
    fn clear_break_command_is_framed() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"clearBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.send_command(BackendCommand::ClearBreak)?;

        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::ClearBreak]);
        Ok(())
    }

    #[test]
    fn command_error_is_reported_with_command_context() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"error","message":"event tap unavailable"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        let Err(error) = session.send_command(BackendCommand::StartBreak(ScheduledBreak {
            name: String::from("short"),
            origin: BreakOrigin::Manual,
            duration: Duration::from_millis(1_500),
            message: String::from("Rest your eyes"),
            autolock: false,
        })) else {
            panic!("helper command error must fail");
        };

        let message = error.to_string();
        assert!(message.contains("startBreak command"));
        assert!(message.contains("event tap unavailable"));
        assert_eq!(daemon_messages(&output)?.len(), 1);
        Ok(())
    }

    #[test]
    fn mismatched_command_completion_is_rejected() {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"finishBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        let Err(error) = session.send_command(BackendCommand::ClearBreak) else {
            panic!("wrong command completion must fail");
        };

        let message = error.to_string();
        assert!(message.contains("expected helper command completion for clearBreak"));
        assert!(message.contains("got finishBreak"));
    }

    #[test]
    fn shutdown_command_is_framed() -> Result<(), Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        let mut session = HelperSession::new(Cursor::new(Vec::new()), &mut output);

        session.send_shutdown()?;

        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::Shutdown]);
        Ok(())
    }

    #[test]
    fn shutdown_waits_for_completion() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"shutdownComplete"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        session.shutdown()?;

        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::Shutdown]);
        Ok(())
    }

    #[test]
    fn shutdown_rejects_wrong_completion() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"clearBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut session = HelperSession::new(input, &mut output);

        let Err(error) = session.shutdown() else {
            panic!("wrong shutdown completion must fail");
        };

        let message = error.to_string();
        assert!(message.contains("expected helper shutdown completion"));
        assert!(message.contains("got commandComplete"));
        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::Shutdown]);
        Ok(())
    }

    #[test]
    fn shutdown_complete_message_is_decoded() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"shutdownComplete"}"#.to_vec());
        let mut session = HelperSession::new(input, Vec::new());

        assert_eq!(session.receive()?, HelperMessage::ShutdownComplete);
        Ok(())
    }

    #[test]
    fn activity_sample_message_is_decoded() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":1000,"lockAfterBreakRequested":false}"#.to_vec(),
        );
        let mut session = HelperSession::new(input, Vec::new());

        assert_eq!(
            session.receive()?,
            HelperMessage::ActivitySample {
                idle_ms: 1_000,
                lock_after_break_requested: false,
                force_exit_requested: false,
                force_exit_reason: None,
                overlay_state: None,
            }
        );
        Ok(())
    }

    #[test]
    fn activity_sample_force_exit_fields_are_decoded() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":1000,"lockAfterBreakRequested":false,"forceExitRequested":true,"forceExitReason":"button","overlayState":"inactive"}"#.to_vec(),
        );
        let mut session = HelperSession::new(input, Vec::new());

        assert_eq!(
            session.receive()?,
            HelperMessage::ActivitySample {
                idle_ms: 1_000,
                lock_after_break_requested: false,
                force_exit_requested: true,
                force_exit_reason: Some(String::from("button")),
                overlay_state: Some(String::from("inactive")),
            }
        );
        Ok(())
    }

    #[test]
    fn active_break_sample_tracks_remaining_time_and_lock_request() {
        let mut active_break = ActiveBreak::new(Duration::from_secs(2), false);

        let update = active_break.apply_sample(
            &HelperActivitySample {
                activity: ActivitySample::new(Duration::from_secs(2)),
                lock_after_break_requested: true,
                force_exit_requested: false,
                force_exit_reason: None,
                overlay_state: None,
            },
            Duration::from_secs(1),
        );

        assert_eq!(
            update,
            ActiveBreakUpdate {
                remaining: Duration::from_secs(1),
                lock_after_break: true,
                lock_after_break_requested: true,
                finished: false,
            }
        );
    }

    #[test]
    fn finished_overlay_sample_finishes_helper_without_zero_update()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":1000,"lockAfterBreakRequested":false}
{"type":"commandComplete","command":"finishBreak"}"#
                .to_vec(),
        );
        let mut output = Vec::new();
        let mut core = MacOSHelperCore::new(
            HelperSession::new(input, &mut output),
            MacOSLock::PlatformDefault,
            BreakDiagnostics::disabled(),
        );
        core.active_break = Some(ActiveBreak::new(OVERLAY_TICK_INTERVAL, false));

        core.sample_overlay_once()?;

        assert_eq!(core.active_break, None);
        assert_eq!(
            core.next_event(),
            Some(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL))
        );
        assert_eq!(core.next_event(), Some(RuntimeEvent::BreakFinished));
        assert_eq!(core.next_event(), None);

        // The runtime still sends FinishBreak after consuming BreakFinished;
        // the backend must ignore it because the helper was already cleared.
        core.finish_break(false);
        drop(core);

        assert_eq!(
            daemon_messages(&output)?,
            vec![
                DaemonMessage::PollActivity,
                DaemonMessage::FinishBreak { lock_after: false },
            ]
        );
        Ok(())
    }

    #[test]
    fn finished_overlay_sample_preserves_lock_request() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":1000,"lockAfterBreakRequested":true}
{"type":"commandComplete","command":"finishBreak"}"#
                .to_vec(),
        );
        let mut output = Vec::new();
        let mut core = MacOSHelperCore::new(
            HelperSession::new(input, &mut output),
            MacOSLock::PlatformDefault,
            BreakDiagnostics::disabled(),
        );
        core.active_break = Some(ActiveBreak::new(OVERLAY_TICK_INTERVAL, false));

        core.sample_overlay_once()?;

        assert_eq!(
            core.next_event(),
            Some(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL))
        );
        assert_eq!(core.next_event(), Some(RuntimeEvent::LockAfterCurrentBreak));
        assert_eq!(core.next_event(), Some(RuntimeEvent::BreakFinished));
        assert_eq!(core.next_event(), None);
        drop(core);

        assert_eq!(
            daemon_messages(&output)?,
            vec![
                DaemonMessage::PollActivity,
                DaemonMessage::FinishBreak { lock_after: true },
            ]
        );
        Ok(())
    }

    #[test]
    fn force_exit_activity_sample_finishes_backend_break() -> Result<(), Box<dyn std::error::Error>>
    {
        let input = Cursor::new(
            br#"{"type":"activitySample","idleMs":1000,"lockAfterBreakRequested":false,"forceExitRequested":true,"forceExitReason":"button","overlayState":"inactive"}"#.to_vec(),
        );
        let mut output = Vec::new();
        let mut core = MacOSHelperCore::new(
            HelperSession::new(input, &mut output),
            MacOSLock::PlatformDefault,
            BreakDiagnostics::disabled(),
        );
        core.active_break = Some(ActiveBreak::new(Duration::from_secs(5), true));

        core.sample_overlay_once()?;

        assert_eq!(core.active_break, None);
        assert_eq!(core.next_event(), Some(RuntimeEvent::BreakFinished));
        assert_eq!(core.next_event(), None);
        drop(core);

        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::PollActivity]);
        Ok(())
    }

    #[test]
    fn force_clear_trigger_is_world_writable_and_consumes_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = unique_force_clear_path("trigger");
        let mut trigger = ForceClearTrigger::prepare(path.clone())?;

        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o666
        );
        assert!(!trigger.consume_if_changed()?);

        append_force_clear(&path)?;

        assert!(trigger.consume_if_changed()?);
        assert_eq!(std::fs::metadata(&path)?.len(), 0);
        assert!(!trigger.consume_if_changed()?);

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn force_clear_file_trigger_clears_active_backend_break()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = unique_force_clear_path("backend");
        let trigger = ForceClearTrigger::prepare(path.clone())?;
        append_force_clear(&path)?;

        let input = Cursor::new(br#"{"type":"commandComplete","command":"clearBreak"}"#.to_vec());
        let mut output = Vec::new();
        let mut core = MacOSHelperCore::new(
            HelperSession::new(input, &mut output),
            MacOSLock::PlatformDefault,
            BreakDiagnostics {
                enabled: true,
                force_clear: Some(trigger),
            },
        );
        core.active_break = Some(ActiveBreak::new(Duration::from_secs(5), true));

        core.sample_overlay_once()?;

        assert_eq!(core.active_break, None);
        assert_eq!(core.next_event(), Some(RuntimeEvent::BreakFinished));
        assert_eq!(core.next_event(), None);
        drop(core);

        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::ClearBreak]);
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn overlay_runtime_events_queue_wall_clock_before_lock_request_and_finish() {
        let mut poller = ActivityPoller::new(OVERLAY_TICK_INTERVAL);

        queue_overlay_runtime_events(
            &mut poller,
            ActiveBreakUpdate {
                remaining: Duration::ZERO,
                lock_after_break: true,
                lock_after_break_requested: true,
                finished: true,
            },
        );

        assert_eq!(
            poller.next_event(),
            Some(RuntimeEvent::WallClockElapsed(OVERLAY_TICK_INTERVAL))
        );
        assert_eq!(
            poller.next_event(),
            Some(RuntimeEvent::LockAfterCurrentBreak)
        );
        assert_eq!(poller.next_event(), Some(RuntimeEvent::BreakFinished));
        assert_eq!(poller.next_event(), None);
    }

    #[test]
    fn command_complete_message_is_decoded() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"commandComplete","command":"startBreak"}"#.to_vec());
        let mut session = HelperSession::new(input, Vec::new());

        assert_eq!(
            session.receive()?,
            HelperMessage::CommandComplete {
                command: HelperCommand::Start,
            }
        );
        Ok(())
    }

    fn daemon_messages(output: &[u8]) -> Result<Vec<DaemonMessage>, Box<dyn std::error::Error>> {
        let output = std::str::from_utf8(output)?;
        output
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    fn daemon_json_values(
        output: &[u8],
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
        let output = std::str::from_utf8(output)?;
        output
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    fn unique_force_clear_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        env::temp_dir().join(format!(
            "rusteyes-force-clear-test-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    fn append_force_clear(path: &Path) -> io::Result<()> {
        OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(b"force\n")
    }

    /// A reader that blocks until its paired sender is dropped, then reports EOF.
    /// Models a wedged helper whose pipe stays open but never produces output.
    struct BlockingReader {
        signal: flume::Receiver<()>,
    }

    impl Read for BlockingReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            // Block until the sender is dropped; then return EOF.
            let _ = self.signal.recv();
            Ok(0)
        }
    }

    #[test]
    fn receive_reports_timeout_when_helper_is_silent() {
        let (keep_open, signal) = flume::unbounded::<()>();
        let mut session = HelperSession::with_read_timeout(
            BlockingReader { signal },
            Vec::new(),
            Duration::from_millis(50),
        );

        let Err(error) = session.receive() else {
            panic!("a silent helper must time out");
        };
        assert!(
            error.to_string().contains("did not respond"),
            "unexpected error: {error}"
        );

        // Hold the sender open until after the read attempt so the reader thread
        // genuinely blocks rather than hitting EOF.
        drop(keep_open);
    }
}
