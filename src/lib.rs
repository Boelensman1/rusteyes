use std::fmt;

mod activity;
mod backend;
pub(crate) mod config;
mod lock_command;

#[cfg(target_os = "macos")]
mod macos_helper;
mod runtime;
pub(crate) mod scheduler;
#[allow(dead_code)]
mod sync_discovery;
#[allow(dead_code)]
pub(crate) mod sync_protocol;
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

    const fn sync_discovery(error: sync_discovery::SyncDiscoveryError) -> Self {
        Self {
            kind: ErrorKind::SyncDiscovery(error),
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
    SyncDiscovery(sync_discovery::SyncDiscoveryError),
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
            ErrorKind::SyncDiscovery(error) => write!(formatter, "{error}"),
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
            ErrorKind::SyncDiscovery(error) => Some(error),
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

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when no platform backend is available, or when startup
/// config loading or scheduler setup fails.
pub fn run() -> Result<(), Error> {
    if sync_discovery::smoke_enabled_from_env() {
        let config = config::Config::load()?;
        return sync_discovery::run_smoke(config.sync).map_err(Error::sync_discovery);
    }

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
