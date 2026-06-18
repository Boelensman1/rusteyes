pub mod config;

mod runtime;
// The scheduler is implemented before the daemon runtime wires it to activity.
#[allow(dead_code)]
pub(crate) mod scheduler;

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when startup config loading fails.
pub fn run() -> Result<(), config::ConfigLoadError> {
    runtime::run()
}
