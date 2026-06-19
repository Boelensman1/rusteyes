use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use tracing_subscriber::fmt::MakeWriter;

fn main() {
    init_logging();

    if let Err(error) = resteyes::run() {
        eprintln!("resteyes: {error}");
        std::process::exit(1);
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(DevStderr)
        .init();
}

#[derive(Debug, Clone, Copy)]
struct DevStderr;

impl<'a> MakeWriter<'a> for DevStderr {
    type Writer = DevStderrWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DevStderrWriter::new()
    }
}

struct DevStderrWriter {
    file: Option<File>,
}

impl DevStderrWriter {
    fn new() -> Self {
        Self {
            file: OpenOptions::new().write(true).open("/dev/stderr").ok(),
        }
    }
}

impl Write for DevStderrWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match &mut self.file {
            Some(file) => file.write(buffer),
            None => Ok(buffer.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}
