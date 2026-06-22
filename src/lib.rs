use std::fmt;

mod activity;
mod backend;
pub(crate) mod config;
mod lock_command;

#[cfg(target_os = "macos")]
mod macos_helper;
mod runtime;
pub(crate) mod scheduler;
mod sync_discovery;
pub(crate) mod sync_protocol;
mod sync_transport;
mod sync_transport_io;
mod ui;
#[cfg(target_os = "linux")]
mod x11_activity;
#[cfg(target_os = "linux")]
mod x11_overlay;

/// Application-level errors returned by `RustEyes`.
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

    #[cfg(target_os = "macos")]
    const fn macos_helper(error: macos_helper::MacOSHelperError) -> Self {
        Self {
            kind: ErrorKind::MacOSHelper(error),
        }
    }

    #[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
    const fn unsupported_platform() -> Self {
        Self {
            kind: ErrorKind::UnsupportedPlatform {
                platform: std::env::consts::OS,
            },
        }
    }

    const fn sync_transport(error: sync_transport::SyncTransportError) -> Self {
        Self {
            kind: ErrorKind::SyncTransport(error),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    const fn ui(error: ui::UiError) -> Self {
        Self {
            kind: ErrorKind::Ui(error),
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    Config(config::ConfigLoadError),
    Schedule(config::ConfigError),
    #[cfg(target_os = "linux")]
    Backend(x11_activity::X11ActivityError),
    #[cfg(target_os = "macos")]
    MacOSHelper(macos_helper::MacOSHelperError),
    SyncTransport(sync_transport::SyncTransportError),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Ui(ui::UiError),
    #[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
    UnsupportedPlatform {
        platform: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Config(error) => write!(formatter, "{error}"),
            ErrorKind::Schedule(error) => write!(formatter, "invalid break schedule: {error}"),
            #[cfg(target_os = "linux")]
            ErrorKind::Backend(error) => write!(formatter, "{error}"),
            #[cfg(target_os = "macos")]
            ErrorKind::MacOSHelper(error) => write!(formatter, "{error}"),
            ErrorKind::SyncTransport(error) => write!(formatter, "{error}"),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            ErrorKind::Ui(error) => write!(formatter, "{error}"),
            #[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
            ErrorKind::UnsupportedPlatform { platform } => {
                write!(formatter, "no backend is available for {platform} yet")
            }
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
            #[cfg(target_os = "macos")]
            ErrorKind::MacOSHelper(error) => Some(error),
            ErrorKind::SyncTransport(error) => Some(error),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            ErrorKind::Ui(error) => Some(error),
            #[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
            ErrorKind::UnsupportedPlatform { .. } => None,
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

impl From<sync_transport::SyncTransportError> for Error {
    fn from(error: sync_transport::SyncTransportError) -> Self {
        Self::sync_transport(error)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl From<ui::UiError> for Error {
    fn from(error: ui::UiError) -> Self {
        Self::ui(error)
    }
}

#[cfg(target_os = "linux")]
impl From<x11_activity::X11ActivityError> for Error {
    fn from(error: x11_activity::X11ActivityError) -> Self {
        Self::backend(error)
    }
}

#[cfg(target_os = "macos")]
impl From<macos_helper::MacOSHelperError> for Error {
    fn from(error: macos_helper::MacOSHelperError) -> Self {
        Self::macos_helper(error)
    }
}

/// Runs the `RustEyes` application.
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
