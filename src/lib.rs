use std::fmt;

pub(crate) mod config;

mod runtime;
// The scheduler is implemented before the daemon runtime wires it to activity.
#[allow(dead_code)]
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
}

#[derive(Debug)]
enum ErrorKind {
    Config(config::ConfigLoadError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Config(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Config(error) => Some(error),
        }
    }
}

impl From<config::ConfigLoadError> for Error {
    fn from(error: config::ConfigLoadError) -> Self {
        Self::config(error)
    }
}

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when startup config loading fails.
pub fn run() -> Result<(), Error> {
    runtime::run().map_err(Error::from)
}
