use crate::backend::{Backend, BackendCommand, RuntimeEvent};
use crate::scheduler::{BreakOrigin, ScheduledBreak};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{trace, warn};

const PROTOCOL_VERSION: u16 = 2;
const HELPER_PATH_ENV: &str = "RESTEYES_MACOS_HELPER";
const DEVELOPMENT_HELPER_PATH: &str = "helpers/macos-helper/.build/debug/resteyes-macos-helper";
const HELPER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const HELPER_SHUTDOWN_POLL: Duration = Duration::from_millis(20);
const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[allow(clippy::module_name_repetitions)]
pub(crate) struct MacOSHelperBackend {
    child: Child,
    session: HelperSession<ChildStdout, ChildStdin>,
    poller: ActivityPoller,
    shutdown_sent: bool,
}

impl MacOSHelperBackend {
    pub(crate) fn connect() -> Result<Self, MacOSHelperError> {
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

        Ok(Self {
            child,
            session,
            poller: ActivityPoller::new(ACTIVITY_POLL_INTERVAL),
            shutdown_sent: false,
        })
    }

    fn shutdown_helper(&mut self) -> Result<(), MacOSHelperError> {
        let mut first_error = None;

        if !self.shutdown_sent {
            match self.session.send_shutdown() {
                Ok(()) => {
                    self.shutdown_sent = true;
                }
                Err(error) => remember_first_error(&mut first_error, error),
            }
        }

        if let Err(error) = self.wait_for_helper_exit() {
            remember_first_error(&mut first_error, error);
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn wait_for_helper_exit(&mut self) -> Result<(), MacOSHelperError> {
        let started = Instant::now();

        while started.elapsed() < HELPER_SHUTDOWN_TIMEOUT {
            if let Some(status) = self.child.try_wait().map_err(|error| {
                MacOSHelperError::new(format!("failed to poll helper exit: {error}"))
            })? {
                trace!(%status, "macOS helper exited");
                return Ok(());
            }

            thread::sleep(HELPER_SHUTDOWN_POLL);
        }

        let mut first_error = Some(MacOSHelperError::new(format!(
            "helper did not exit within {} ms after shutdown",
            duration_millis(HELPER_SHUTDOWN_TIMEOUT)
        )));

        if let Err(error) = self.child.kill() {
            remember_first_error(
                &mut first_error,
                MacOSHelperError::new(format!("failed to kill unresponsive helper: {error}")),
            );
        }

        if let Err(error) = self.child.wait() {
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
}

impl Backend for MacOSHelperBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        loop {
            if let Some(event) = self.poller.next_event() {
                return event;
            }

            thread::sleep(self.poller.poll_interval());

            match self.session.poll_activity() {
                Ok(sample) => {
                    self.poller.queue_sample(sample);
                }
                Err(error) => {
                    warn!(%error, "failed to poll macOS activity");
                    return RuntimeEvent::Shutdown;
                }
            }
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        if let Err(error) = self.session.send_command(command) {
            warn!(%error, "failed to send command to macOS helper");
        }
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

        match self.receive()? {
            HelperMessage::Ready { version } if version == PROTOCOL_VERSION => Ok(()),
            HelperMessage::Ready { version } => Err(MacOSHelperError::new(format!(
                "helper protocol version {version} is incompatible with daemon protocol version {PROTOCOL_VERSION}"
            ))),
            message @ HelperMessage::ActivitySample { .. } => Err(MacOSHelperError::new(format!(
                "expected helper ready message during handshake, got {}",
                message.message_type()
            ))),
            HelperMessage::Error { message } => Err(MacOSHelperError::new(format!(
                "helper reported handshake error: {message}"
            ))),
            HelperMessage::ShutdownComplete => Err(MacOSHelperError::new(format!(
                "expected helper ready message during handshake, got {}",
                HelperMessage::ShutdownComplete.message_type()
            ))),
        }
    }

    fn send_command(&mut self, command: BackendCommand) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::from(command))
    }

    fn poll_activity(&mut self) -> Result<ActivitySample, MacOSHelperError> {
        self.send(&DaemonMessage::PollActivity)?;

        match self.receive()? {
            HelperMessage::ActivitySample { idle_ms } => {
                Ok(ActivitySample::new(Duration::from_millis(idle_ms)))
            }
            HelperMessage::Error { message } => Err(MacOSHelperError::new(format!(
                "helper reported activity error: {message}"
            ))),
            message => Err(MacOSHelperError::new(format!(
                "expected helper activity sample, got {}",
                message.message_type()
            ))),
        }
    }

    fn send_shutdown(&mut self) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::Shutdown)
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DaemonMessage {
    Hello {
        version: u16,
    },
    StartBreak {
        #[serde(rename = "break")]
        break_info: WireBreak,
    },
    FinishBreak {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum HelperMessage {
    Ready {
        version: u16,
    },
    ActivitySample {
        #[serde(rename = "idleMs")]
        idle_ms: u64,
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
            Self::ShutdownComplete => "shutdownComplete",
            Self::Error { .. } => "error",
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

    const fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn queue_sample(&mut self, sample: ActivitySample) -> ActivityState {
        let state = sample.state_for(self.poll_interval);
        trace!(
            idle_for = ?sample.idle_for,
            ?state,
            poll_interval = ?self.poll_interval,
            "sampled macOS activity"
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
        let input = Cursor::new(br#"{"type":"ready","version":2}"#.to_vec());
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
    fn handshake_rejects_incompatible_version() {
        let input = Cursor::new(br#"{"type":"ready","version":3}"#.to_vec());
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
        let input = Cursor::new(br#"{"type":"activitySample","idleMs":750}"#.to_vec());
        let mut output = Vec::new();
        let sample = {
            let mut session = HelperSession::new(input, &mut output);
            session.poll_activity()?
        };

        assert_eq!(sample.idle_for, Duration::from_millis(750));
        assert_eq!(daemon_messages(&output)?, vec![DaemonMessage::PollActivity]);
        Ok(())
    }

    #[test]
    fn active_sample_queues_wall_clock_before_active_time() {
        let poll_interval = Duration::from_secs(1);
        let mut poller = ActivityPoller::new(poll_interval);

        assert_eq!(
            poller.queue_sample(ActivitySample::new(Duration::from_millis(500))),
            ActivityState::Active
        );
        assert_eq!(
            poller.next_event(),
            Some(RuntimeEvent::WallClockElapsed(poll_interval))
        );
        assert_eq!(
            poller.next_event(),
            Some(RuntimeEvent::ActiveTimeElapsed(poll_interval))
        );
        assert_eq!(poller.next_event(), None);
    }

    #[test]
    fn idle_sample_queues_only_wall_clock_time() {
        let poll_interval = Duration::from_secs(1);
        let mut poller = ActivityPoller::new(poll_interval);

        assert_eq!(
            poller.queue_sample(ActivitySample::new(Duration::from_millis(1_001))),
            ActivityState::Idle
        );
        assert_eq!(
            poller.next_event(),
            Some(RuntimeEvent::WallClockElapsed(poll_interval))
        );
        assert_eq!(poller.next_event(), None);
    }

    #[test]
    fn start_break_command_uses_millisecond_wire_duration() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut output = Vec::new();
        let mut session = HelperSession::new(Cursor::new(Vec::new()), &mut output);

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
    fn shutdown_command_is_framed() -> Result<(), Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        let mut session = HelperSession::new(Cursor::new(Vec::new()), &mut output);

        session.send_shutdown()?;

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
        let input = Cursor::new(br#"{"type":"activitySample","idleMs":1000}"#.to_vec());
        let mut session = HelperSession::new(input, Vec::new());

        assert_eq!(
            session.receive()?,
            HelperMessage::ActivitySample { idle_ms: 1_000 }
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
}
