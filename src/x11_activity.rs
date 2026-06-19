use crate::backend::{Backend, BackendCommand, RuntimeEvent};
use std::collections::VecDeque;
use std::fmt;
use std::thread;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const SCREENSAVER_CLIENT_MAJOR_VERSION: u8 = 1;
const SCREENSAVER_CLIENT_MINOR_VERSION: u8 = 1;

#[allow(clippy::module_name_repetitions)]
pub(crate) struct X11ActivityBackend {
    activity: X11Activity,
    poller: ActivityPoller,
}

impl X11ActivityBackend {
    pub(crate) fn connect() -> Result<Self, X11ActivityError> {
        Ok(Self {
            activity: X11Activity::connect()?,
            poller: ActivityPoller::new(POLL_INTERVAL),
        })
    }

    fn next_detail(&mut self) -> Result<X11ActivityDetail, X11ActivityError> {
        if let Some(event) = self.poller.next_event() {
            Ok(X11ActivityDetail::Runtime(event))
        } else {
            self.poll_once()
        }
    }

    fn poll_once(&mut self) -> Result<X11ActivityDetail, X11ActivityError> {
        thread::sleep(self.poller.poll_interval());

        let sample = self.activity.sample()?;
        let state = self.poller.queue_sample(sample);

        Ok(X11ActivityDetail::Sample {
            sample,
            state,
            poll_interval: self.poller.poll_interval(),
        })
    }
}

impl Backend for X11ActivityBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        loop {
            match self.next_detail() {
                Ok(X11ActivityDetail::Runtime(event)) => return event,
                Ok(X11ActivityDetail::Sample { .. }) => {}
                Err(_) => return RuntimeEvent::Shutdown,
            }
        }
    }

    fn handle_command(&mut self, _command: BackendCommand) {}
}

#[allow(clippy::module_name_repetitions)]
pub(crate) struct DiagnosticX11ActivityBackend {
    inner: X11ActivityBackend,
}

impl DiagnosticX11ActivityBackend {
    pub(crate) fn connect() -> Result<Self, X11ActivityError> {
        Ok(Self {
            inner: X11ActivityBackend::connect()?,
        })
    }
}

impl Backend for DiagnosticX11ActivityBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        loop {
            match self.inner.next_detail() {
                Ok(X11ActivityDetail::Runtime(event)) => return event,
                Ok(X11ActivityDetail::Sample {
                    sample,
                    state,
                    poll_interval,
                }) => eprintln!("{}", format_diagnostic_sample(sample, state, poll_interval)),
                Err(error) => {
                    eprintln!("resteyes: x11 activity error: {error}");
                    return RuntimeEvent::Shutdown;
                }
            }
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        eprintln!("resteyes: backend command: {command:?}");
        self.inner.handle_command(command);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum X11ActivityDetail {
    Runtime(RuntimeEvent),
    Sample {
        sample: ActivitySample,
        state: ActivityState,
        poll_interval: Duration,
    },
}

struct X11Activity {
    connection: RustConnection,
    root: Window,
}

impl X11Activity {
    fn connect() -> Result<Self, X11ActivityError> {
        let (connection, screen_index) =
            x11rb::connect(None).map_err(|error| X11ActivityError::connect(error.to_string()))?;
        let root = connection.setup().roots[screen_index].root;

        connection
            .screensaver_query_version(
                SCREENSAVER_CLIENT_MAJOR_VERSION,
                SCREENSAVER_CLIENT_MINOR_VERSION,
            )
            .map_err(|error| X11ActivityError::query_version(error.to_string()))?
            .reply()
            .map_err(|error| X11ActivityError::query_version(error.to_string()))?;

        Ok(Self { connection, root })
    }

    fn sample(&self) -> Result<ActivitySample, X11ActivityError> {
        let reply = self
            .connection
            .screensaver_query_info(self.root)
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

        self.events
            .push_back(RuntimeEvent::WallClockElapsed(self.poll_interval));

        if state == ActivityState::Active {
            self.events
                .push_back(RuntimeEvent::ActiveTimeElapsed(self.poll_interval));
        }

        state
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

impl ActivityState {
    const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
        }
    }
}

fn format_diagnostic_sample(
    sample: ActivitySample,
    state: ActivityState,
    poll_interval: Duration,
) -> String {
    format!(
        "resteyes: x11 activity state={} idle_ms={} tick_ms={}",
        state.label(),
        sample.idle_for.as_millis(),
        poll_interval.as_millis()
    )
}

#[cfg(test)]
mod tests;
