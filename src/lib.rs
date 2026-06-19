use std::fmt;

#[cfg(any(test, target_os = "linux"))]
mod backend;
#[cfg(any(test, target_os = "linux"))]
pub(crate) mod config;

mod runtime;
#[cfg(any(test, target_os = "linux"))]
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
    #[cfg(any(test, target_os = "linux"))]
    const fn config(error: config::ConfigLoadError) -> Self {
        Self {
            kind: ErrorKind::Config(error),
        }
    }

    #[cfg(any(test, target_os = "linux"))]
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

    #[cfg(any(test, not(target_os = "linux")))]
    const fn unsupported_platform() -> Self {
        Self {
            kind: ErrorKind::UnsupportedPlatform {
                platform: std::env::consts::OS,
            },
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    #[cfg(any(test, target_os = "linux"))]
    Config(config::ConfigLoadError),
    #[cfg(any(test, target_os = "linux"))]
    Schedule(config::ConfigError),
    #[cfg(target_os = "linux")]
    Backend(x11_activity::X11ActivityError),
    #[cfg(any(test, not(target_os = "linux")))]
    UnsupportedPlatform { platform: &'static str },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            #[cfg(any(test, target_os = "linux"))]
            ErrorKind::Config(error) => write!(formatter, "{error}"),
            #[cfg(any(test, target_os = "linux"))]
            ErrorKind::Schedule(error) => write!(formatter, "invalid break schedule: {error}"),
            #[cfg(target_os = "linux")]
            ErrorKind::Backend(error) => write!(formatter, "{error}"),
            #[cfg(any(test, not(target_os = "linux")))]
            ErrorKind::UnsupportedPlatform { platform } => {
                write!(formatter, "no backend is available for {platform} yet")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            #[cfg(any(test, target_os = "linux"))]
            ErrorKind::Config(error) => Some(error),
            #[cfg(any(test, target_os = "linux"))]
            ErrorKind::Schedule(error) => Some(error),
            #[cfg(target_os = "linux")]
            ErrorKind::Backend(error) => Some(error),
            #[cfg(any(test, not(target_os = "linux")))]
            ErrorKind::UnsupportedPlatform { .. } => None,
        }
    }
}

#[cfg(any(test, target_os = "linux"))]
impl From<config::ConfigLoadError> for Error {
    fn from(error: config::ConfigLoadError) -> Self {
        Self::config(error)
    }
}

#[cfg(any(test, target_os = "linux"))]
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
/// Returns an error when no platform backend is available, or when startup
/// config loading or scheduler setup fails.
pub fn run() -> Result<(), Error> {
    runtime::run()
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn unsupported_platform_error_explains_missing_backend() {
        assert_eq!(
            Error::unsupported_platform().to_string(),
            format!("no backend is available for {} yet", std::env::consts::OS)
        );
    }
}
