use std::fmt;

pub(crate) mod config;

mod runtime;
pub(crate) mod scheduler;

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
}

#[derive(Debug)]
enum ErrorKind {
    Config(config::ConfigLoadError),
    Schedule(config::ConfigError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Config(error) => write!(formatter, "{error}"),
            ErrorKind::Schedule(error) => write!(formatter, "invalid break schedule: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Config(error) => Some(error),
            ErrorKind::Schedule(error) => Some(error),
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

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when startup config loading or scheduler setup fails.
pub fn run() -> Result<(), Error> {
    runtime::run()
}
