use crate::backend::{Backend, BackendCommand, DisableRequest, RuntimeEvent};
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

const PROTOCOL_VERSION: u16 = 1;
const HELPER_PATH_ENV: &str = "RESTEYES_MACOS_HELPER";
const DEVELOPMENT_HELPER_PATH: &str = "helpers/macos-helper/.build/debug/resteyes-macos-helper";
const HELPER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const HELPER_SHUTDOWN_POLL: Duration = Duration::from_millis(20);

#[allow(clippy::module_name_repetitions)]
pub(crate) struct MacOSHelperBackend {
    child: Child,
    session: HelperSession<ChildStdout, ChildStdin>,
    shutdown_sent: bool,
}

impl MacOSHelperBackend {
    pub(crate) fn connect() -> Result<Self, MacOSHelperError> {
        let path = helper_path();
        let mut child = spawn_helper(&path)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MacOSHelperError::process("helper stdin was unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MacOSHelperError::process("helper stdout was unavailable"))?;

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
            shutdown_sent: false,
        })
    }

    fn shutdown_helper(&mut self) -> Result<(), MacOSHelperError> {
        if !self.shutdown_sent {
            self.session.send_shutdown()?;
            self.shutdown_sent = true;
        }

        self.wait_for_helper_exit()
    }

    fn wait_for_helper_exit(&mut self) -> Result<(), MacOSHelperError> {
        let started = Instant::now();

        while started.elapsed() < HELPER_SHUTDOWN_TIMEOUT {
            if let Some(status) = self.child.try_wait().map_err(|error| {
                MacOSHelperError::process(format!("failed to poll helper exit: {error}"))
            })? {
                trace!(%status, "macOS helper exited");
                return Ok(());
            }

            thread::sleep(HELPER_SHUTDOWN_POLL);
        }

        self.child.kill().map_err(|error| {
            MacOSHelperError::process(format!("failed to kill unresponsive helper: {error}"))
        })?;
        let _ = self.child.wait();
        Err(MacOSHelperError::process(format!(
            "helper did not exit within {} ms after shutdown",
            duration_millis(HELPER_SHUTDOWN_TIMEOUT)
        )))
    }
}

impl Backend for MacOSHelperBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        RuntimeEvent::Shutdown
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
    fn process(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
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
            HelperMessage::Ready { version } => Err(MacOSHelperError::protocol(format!(
                "helper protocol version {version} is incompatible with daemon protocol version {PROTOCOL_VERSION}"
            ))),
            HelperMessage::Error { message } => Err(MacOSHelperError::protocol(format!(
                "helper reported handshake error: {message}"
            ))),
            message => Err(MacOSHelperError::protocol(format!(
                "expected helper ready message during handshake, got {}",
                message.message_type()
            ))),
        }
    }

    fn send_command(&mut self, command: BackendCommand) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::from(command))
    }

    fn send_shutdown(&mut self) -> Result<(), MacOSHelperError> {
        self.send(&DaemonMessage::Shutdown)
    }

    fn send(&mut self, message: &DaemonMessage) -> Result<(), MacOSHelperError> {
        serde_json::to_writer(&mut self.writer, message).map_err(|error| {
            MacOSHelperError::protocol(format!("failed to encode helper message: {error}"))
        })?;
        self.writer.write_all(b"\n").map_err(|error| {
            MacOSHelperError::process(format!("failed to write helper message: {error}"))
        })?;
        self.writer.flush().map_err(|error| {
            MacOSHelperError::process(format!("failed to flush helper message: {error}"))
        })
    }

    fn receive(&mut self) -> Result<HelperMessage, MacOSHelperError> {
        let mut line = String::new();
        let bytes = self.reader.read_line(&mut line).map_err(|error| {
            MacOSHelperError::process(format!("failed to read helper message: {error}"))
        })?;

        if bytes == 0 {
            return Err(MacOSHelperError::process(
                "helper closed protocol output before sending a message",
            ));
        }

        serde_json::from_str(line.trim_end()).map_err(|error| {
            MacOSHelperError::protocol(format!("failed to decode helper message: {error}"))
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

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum HelperMessage {
    Ready { version: u16 },
    ActiveTimeElapsed { duration_ms: u64 },
    WallClockElapsed { duration_ms: u64 },
    BreakFinished,
    LockAfterCurrentBreak,
    StartManualBreak { name: String },
    DisableFor { duration_ms: u64 },
    DisableUntilRestart,
    Enable,
    ShutdownComplete,
    Error { message: String },
}

impl HelperMessage {
    const fn message_type(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::ActiveTimeElapsed { .. } => "activeTimeElapsed",
            Self::WallClockElapsed { .. } => "wallClockElapsed",
            Self::BreakFinished => "breakFinished",
            Self::LockAfterCurrentBreak => "lockAfterCurrentBreak",
            Self::StartManualBreak { .. } => "startManualBreak",
            Self::DisableFor { .. } => "disableFor",
            Self::DisableUntilRestart => "disableUntilRestart",
            Self::Enable => "enable",
            Self::ShutdownComplete => "shutdownComplete",
            Self::Error { .. } => "error",
        }
    }
}

impl TryFrom<HelperMessage> for RuntimeEvent {
    type Error = MacOSHelperError;

    fn try_from(message: HelperMessage) -> Result<Self, Self::Error> {
        match message {
            HelperMessage::ActiveTimeElapsed { duration_ms } => {
                Ok(Self::ActiveTimeElapsed(Duration::from_millis(duration_ms)))
            }
            HelperMessage::WallClockElapsed { duration_ms } => {
                Ok(Self::WallClockElapsed(Duration::from_millis(duration_ms)))
            }
            HelperMessage::BreakFinished => Ok(Self::BreakFinished),
            HelperMessage::LockAfterCurrentBreak => Ok(Self::LockAfterCurrentBreak),
            HelperMessage::StartManualBreak { name } => Ok(Self::StartManualBreak(name)),
            HelperMessage::DisableFor { duration_ms } => Ok(Self::Disable(DisableRequest::For(
                Duration::from_millis(duration_ms),
            ))),
            HelperMessage::DisableUntilRestart => Ok(Self::Disable(DisableRequest::UntilRestart)),
            HelperMessage::Enable => Ok(Self::Enable),
            HelperMessage::ShutdownComplete => Ok(Self::Shutdown),
            HelperMessage::Error { message } => Err(MacOSHelperError::protocol(format!(
                "helper reported error: {message}"
            ))),
            HelperMessage::Ready { .. } => Err(MacOSHelperError::protocol(
                "unexpected helper ready message after handshake",
            )),
        }
    }
}

fn spawn_helper(path: &Path) -> Result<Child, MacOSHelperError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MacOSHelperError::process(format!(
            "failed to find helper at {}: {error}",
            path.display()
        ))
    })?;

    if !metadata.is_file() {
        return Err(MacOSHelperError::process(format!(
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
        MacOSHelperError::process(format!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn handshake_writes_hello_and_accepts_ready() -> Result<(), Box<dyn std::error::Error>> {
        let input = Cursor::new(br#"{"type":"ready","version":1}"#.to_vec());
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
        let input = Cursor::new(br#"{"type":"ready","version":2}"#.to_vec());
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
    fn helper_events_convert_to_runtime_events() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            RuntimeEvent::try_from(HelperMessage::ActiveTimeElapsed { duration_ms: 250 })?,
            RuntimeEvent::ActiveTimeElapsed(Duration::from_millis(250))
        );
        assert_eq!(
            RuntimeEvent::try_from(HelperMessage::DisableFor {
                duration_ms: 30_000
            })?,
            RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30)))
        );
        assert_eq!(
            RuntimeEvent::try_from(HelperMessage::ShutdownComplete)?,
            RuntimeEvent::Shutdown
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
