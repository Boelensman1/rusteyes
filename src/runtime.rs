use crate::config::{Config, ConfigLoadError};

pub fn run() -> Result<(), ConfigLoadError> {
    let _config = Config::load()?;

    println!("hello world");
    Ok(())
}
