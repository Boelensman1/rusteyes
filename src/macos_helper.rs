use crate::activity::{ActivityPoller, ActivitySample, BreakTimer, break_elapsed_for_sample};
use crate::backend::{Backend, BackendCommand, RuntimeEvent};
use crate::config::LockConfig;
use crate::lock_command::{LockCommand, LockCommandError, start_lock_command};
use crate::scheduler::{BreakOrigin, ScheduledBreak};
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{trace, warn};

const PROTOCOL_VERSION: u16 = 6;
const HELPER_PATH_ENV: &str = "RESTEYES_MACOS_HELPER";
const DEVELOPMENT_HELPER_PATH: &str = "helpers/macos-helper/.build/debug/resteyes-macos-helper";
const HELPER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const HELPER_SHUTDOWN_POLL: Duration = Duration::from_millis(20);
const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_secs(1);
const OVERLAY_TICK_INTERVAL: Duration = Duration::from_millis(500);

#[allow(clippy::module_name_repetitions)]
pub(crate) struct MacOSHelperBackend {
    child: Child,
    session: HelperSession<ChildStdout, ChildStdin>,
    poller: ActivityPoller,
    active_break: Option<ActiveBreak>,
    lock: MacOSLock,
    shutdown_sent: bool,
}

impl MacOSHelperBackend {
    pub(crate) fn connect(lock_config: LockConfig) -> Result<Self, MacOSHelperError> {
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
            session,
            poller: ActivityPoller::new(ACTIVITY_POLL_INTERVAL),
            active_break: None,
            lock: MacOSLock::from(lock_config),
            shutdown_sent: false,
        })
    }

    fn poll_once(&mut self) -> Result<(), MacOSHelperError> {
        if self.active_break.is_some() {
            self.poll_overlay_once()
        } else {
            self.poll_activity_once()
        }
    }

    fn poll_activity_once(&mut self) -> Result<(), MacOSHelperError> {
        thread::sleep(self.poller.poll_interval());

        let sample = self.session.poll_activity()?;
        self.poller.queue_sample(sample.activity);

        Ok(())
    }

    fn poll_overlay_once(&mut self) -> Result<(), MacOSHelperError> {
        thread::sleep(OVERLAY_TICK_INTERVAL);

        let sample = self.session.poll_activity()?;
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
            .map(|active_break| active_break.apply_sample(sample, break_elapsed))
        {
            self.session
                .update_break(update.remaining, update.lock_after_break)?;
            queue_overlay_runtime_events(&mut self.poller, update);
        }

        Ok(())
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) {
        let duration = scheduled_break.duration;
        let lock_after_break = scheduled_break.autolock;
        if self.send_backend_command(BackendCommand::StartBreak(scheduled_break)) {
            self.active_break = Some(ActiveBreak::new(duration, lock_after_break));
        }
    }

    fn finish_break(&mut self, lock_after: bool) {
        let helper_lock_after = self.lock.helper_lock_after(lock_after);
        match self.session.send_command(BackendCommand::FinishBreak {
            lock_after: helper_lock_after,
        }) {
            Ok(()) => {
                self.active_break = None;
                if lock_after {
                    self.start_configured_lock_command();
                }
            }
            Err(error) => {
                self.active_break = None;
                self.queue_backend_error(&error);
            }
        }
    }

    fn start_configured_lock_command(&mut self) {
        let MacOSLock::Command(lock_command) = &self.lock else {
            return;
        };

        if let Err(error) = start_lock_command(lock_command) {
            self.queue_backend_error(&MacOSHelperError::lock_command(&error));
        }
    }

    fn clear_break(&mut self) {
        if self.send_backend_command(BackendCommand::ClearBreak) {
            self.active_break = None;
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

    fn shutdown_helper(&mut self) -> Result<(), MacOSHelperError> {
        shutdown_helper_process(
            &mut self.child,
            Some(&mut self.session),
            &mut self.shutdown_sent,
        )
    }
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

impl Backend for MacOSHelperBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        loop {
            if let Some(event) = self.poller.next_event() {
                return event;
            }

            match self.poll_once() {
                Ok(()) => {}
                Err(error) => {
                    warn!(%error, "failed to poll macOS activity");
                    return RuntimeEvent::Shutdown;
                }
            }
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        match command {
            BackendCommand::StartBreak(scheduled_break) => self.start_break(scheduled_break),
            BackendCommand::FinishBreak { lock_after } => self.finish_break(lock_after),
            BackendCommand::ClearBreak => self.clear_break(),
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
        sample: HelperActivitySample,
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

impl fmt::Display for MacOSHelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "macOS helper error: {}", self.message)
    }
}

impl std::error::Error for MacOSHelperError {}

struct HelperSession<R, W> {
    reader: BufReader<R>,
    writer: W,
}

impl<R, W> HelperSession<R, W>
where
    R: Read,
    W: Write,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
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
            },
            HelperCommand::Update,
        )
    }

    fn send_helper_command(
        &mut self,
        message: &DaemonMessage,
        expected_command: HelperCommand,
    ) -> Result<(), MacOSHelperError> {
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
        self.send(&DaemonMessage::PollActivity)?;

        self.receive_expected(
            "helper activity sample",
            "activity",
            |message| match message {
                HelperMessage::ActivitySample {
                    idle_ms,
                    lock_after_break_requested,
                } => Ok(HelperActivitySample {
                    activity: ActivitySample::new(Duration::from_millis(idle_ms)),
                    lock_after_break_requested,
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
        let mut line = String::new();
        let bytes = self.reader.read_line(&mut line).map_err(|error| {
            MacOSHelperError::new(format!("failed to read helper message: {error}"))
        })?;

        if bytes == 0 {
            return Err(MacOSHelperError::new(
                "helper closed protocol output before sending a message",
            ));
        }

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
    messages: Vec<String>,
    autolock: bool,
}

impl From<ScheduledBreak> for WireBreak {
    fn from(scheduled_break: ScheduledBreak) -> Self {
        Self {
            name: scheduled_break.name,
            origin: WireBreakOrigin::from(scheduled_break.origin),
            duration_ms: duration_millis(scheduled_break.duration),
            messages: scheduled_break.messages,
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
    const fn from_backend_command(command: &BackendCommand) -> Self {
        match command {
            BackendCommand::StartBreak(_) => Self::Start,
            BackendCommand::FinishBreak { .. } => Self::Finish,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelperActivitySample {
    activity: ActivitySample,
    lock_after_break_requested: bool,
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
            "missing macOS privacy permission{plural}: {missing_permissions}. Open System Settings > Privacy & Security and grant the listed permission{plural} to Resteyes, then restart Resteyes. Development builds may appear as resteyes-macos-helper."
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

fn shutdown_helper_after_startup_error(
    child: &mut Child,
    session: &mut HelperSession<ChildStdout, ChildStdin>,
) {
    let mut shutdown_sent = false;
    if let Err(error) = shutdown_helper_process(child, Some(session), &mut shutdown_sent) {
        warn!(%error, "failed to shut down macOS helper after startup error");
    }
}

fn shutdown_helper_process(
    child: &mut Child,
    session: Option<&mut HelperSession<ChildStdout, ChildStdin>>,
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
        .name(String::from("resteyes-macos-helper-stderr"))
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
                    "resteyes: macOS helper stderr ({description}): {line}"
                );
                trace!(helper = %description, %line, "macOS helper stderr");
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "resteyes: failed to read macOS helper stderr ({description}): {error}"
                );
                trace!(helper = %description, %error, "failed to read macOS helper stderr");
                break;
            }
        }
    }
}

fn helper_path() -> PathBuf {
    env::var_os(HELPER_PATH_ENV).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join(DEVELOPMENT_HELPER_PATH),
        PathBuf::from,
    )
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

    #[test]
    fn handshake_writes_hello_and_accepts_ready() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"ready","version":6}"#.to_vec());
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
        assert!(message.contains("resteyes-macos-helper"));
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
        assert!(message.contains("resteyes-macos-helper"));
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
        assert!(message.contains("restart Resteyes"));
    }

    #[test]
    fn handshake_rejects_incompatible_version() {
        let input = Cursor::new(br#"{"type":"ready","version":7}"#.to_vec());
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
            messages: vec![String::from("Rest your eyes")],
            autolock: true,
        }))?;

        assert_eq!(
            daemon_messages(&output)?,
            vec![DaemonMessage::StartBreak {
                break_info: WireBreak {
                    name: String::from("short"),
                    origin: WireBreakOrigin::Scheduled { slot: 3 },
                    duration_ms: 1_500,
                    messages: vec![String::from("Rest your eyes")],
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
            }]
        );
        let messages = daemon_json_values(&output)?;
        assert_eq!(messages[0]["remainingMs"], 2_500);
        assert_eq!(messages[0]["lockAfter"], true);
        assert!(messages[0].get("lock_after").is_none());
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
            messages: vec![String::from("Rest your eyes")],
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
            }
        );
        Ok(())
    }

    #[test]
    fn active_break_sample_tracks_remaining_time_and_lock_request() {
        let mut active_break = ActiveBreak::new(Duration::from_secs(2), false);

        let update = active_break.apply_sample(
            HelperActivitySample {
                activity: ActivitySample::new(Duration::from_secs(2)),
                lock_after_break_requested: true,
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
}
