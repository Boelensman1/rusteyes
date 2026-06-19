use std::fmt;

mod backend;
pub(crate) mod config;

mod runtime;
pub(crate) mod scheduler;
#[cfg(target_os = "linux")]
mod x11_activity;
#[cfg(target_os = "linux")]
mod x11_overlay;

/// Application-level errors returned by Resteyes.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    const fn config(error: config::ConfigLoadError) -> Self {
        Self {
            kind: ErrorKind::Config(error),
        }
    }

    const fn schedule(error: config::ConfigError) -> Self {
        Self {
            kind: ErrorKind::Schedule(error),
        }
    }

    #[cfg(target_os = "linux")]
    const fn backend(error: x11_activity::X11ActivityError) -> Self {
        Self {
            kind: ErrorKind::Backend(error),
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    Config(config::ConfigLoadError),
    Schedule(config::ConfigError),
    #[cfg(target_os = "linux")]
    Backend(x11_activity::X11ActivityError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Config(error) => write!(formatter, "{error}"),
            ErrorKind::Schedule(error) => write!(formatter, "invalid break schedule: {error}"),
            #[cfg(target_os = "linux")]
            ErrorKind::Backend(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Config(error) => Some(error),
            ErrorKind::Schedule(error) => Some(error),
            #[cfg(target_os = "linux")]
            ErrorKind::Backend(error) => Some(error),
        }
    }
}

impl From<config::ConfigLoadError> for Error {
    fn from(error: config::ConfigLoadError) -> Self {
        Self::config(error)
    }
}

impl From<config::ConfigError> for Error {
    fn from(error: config::ConfigError) -> Self {
        Self::schedule(error)
    }
}

#[cfg(target_os = "linux")]
impl From<x11_activity::X11ActivityError> for Error {
    fn from(error: x11_activity::X11ActivityError) -> Self {
        Self::backend(error)
    }
}

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when startup config loading or scheduler setup fails.
pub fn run() -> Result<(), Error> {
    runtime::run()
}
