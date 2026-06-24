use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::os::fd::AsFd;
use tracing::Level;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::prelude::*;

fn main() {
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting RustEyes");

    if let Err(error) = rusteyes::run() {
        eprintln!("rusteyes: {error}");
        std::process::exit(1);
    }
}

fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    // Route by level so service managers split the streams the conventional way:
    // INFO/DEBUG/TRACE go to stdout (launchd StandardOutPath / journald), while
    // WARN/ERROR go to stderr so the error log stays a clean "something is wrong"
    // record. tracing's Level ordering is TRACE > DEBUG > INFO > WARN > ERROR, so
    // `>= INFO` selects INFO-and-more-verbose and `<= WARN` selects WARN/ERROR;
    // the two bands are disjoint and complete. ANSI colors only when the target
    // is a real terminal, so log files and sockets stay plain text.
    let stdout_layer = fmt::layer()
        .with_ansi(io::stdout().is_terminal())
        .with_writer(StdStream::Stdout)
        .with_filter(filter_fn(|meta| *meta.level() >= Level::INFO));

    let stderr_layer = fmt::layer()
        .with_ansi(io::stderr().is_terminal())
        .with_writer(StdStream::Stderr)
        .with_filter(filter_fn(|meta| *meta.level() <= Level::WARN));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(stderr_layer)
        .init();
}

#[derive(Debug, Clone, Copy)]
enum StdStream {
    Stdout,
    Stderr,
}

impl<'a> MakeWriter<'a> for StdStream {
    type Writer = StdStreamWriter;

    fn make_writer(&'a self) -> Self::Writer {
        // Write through a dup of the inherited fd (1 or 2) rather than the global
        // Rust stdout/stderr writer. Going through std::io::stderr() takes a
        // reentrant lock that, on macOS, blocked the runtime while formatting the
        // first activity trace event; a dup'd handle bypasses that lock.
        //
        // A dup shares the fd's open file description, so it writes correctly to
        // whatever a service manager attaches: a journald socket (systemd), an
        // append file (launchd StandardOutPath/StandardErrorPath), a tty, or a
        // pipe. Reopening /dev/std{out,err} by path instead — the obvious
        // alternative — fails with ENXIO on a socket (so systemd logs were
        // silently dropped) and reopens regular files at offset 0, overwriting
        // earlier lines.
        let cloned = match self {
            StdStream::Stdout => io::stdout().as_fd().try_clone_to_owned(),
            StdStream::Stderr => io::stderr().as_fd().try_clone_to_owned(),
        };
        StdStreamWriter {
            file: cloned.ok().map(File::from),
        }
    }
}

struct StdStreamWriter {
    file: Option<File>,
}

impl Write for StdStreamWriter {
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
