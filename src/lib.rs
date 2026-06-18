pub mod config;

mod runtime;

/// Runs the Resteyes application.
///
/// # Errors
///
/// Returns an error when startup config loading fails.
pub fn run() -> Result<(), config::ConfigLoadError> {
    runtime::run()
}
