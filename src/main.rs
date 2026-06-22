use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsFd;
use tracing_subscriber::fmt::MakeWriter;

fn main() {
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting RustEyes");

    if let Err(error) = rusteyes::run() {
        eprintln!("rusteyes: {error}");
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
        // Write through a dup of the inherited stderr (fd 2) rather than the
        // global Rust stderr writer. Going through std::io::stderr() takes a
        // reentrant lock that, on macOS, blocked the runtime while formatting
        // the first activity trace event; a dup'd handle bypasses that lock.
        //
        // A dup shares fd 2's open file description, so it writes correctly to
        // whatever a service manager attaches: a journald socket (systemd), an
        // append file (launchd StandardErrorPath), a tty, or a pipe. Reopening
        // /dev/stderr by path instead — the obvious alternative — fails with
        // ENXIO on a socket (so systemd logs were silently dropped) and reopens
        // regular files at offset 0, overwriting earlier lines.
        let file = std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .ok()
            .map(File::from);
        Self { file }
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
