//! Command output that survives its reader going away.
//!
//! `println!` panics when stdout is a pipe nobody reads any more (`tumult run
//! ... | head -1`), and a panic is an abort in the release profile: the run
//! would end before its journal is ingested. Commands print through
//! [`outln!`](crate::outln), [`out!`](crate::out) and
//! [`errln!`](crate::errln) instead. A stream that fails is given up on, the
//! command finishes its work, and the exit code says how that went.

use std::fmt::Arguments;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use tumult_otel::logging::err;

/// Prints to stdout like `println!`, without panicking when stdout is
/// closed.
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::output::stdout($crate::__output_line!($($arg)*))
    };
}

/// The arguments of one line of output: what `println!` would print.
#[doc(hidden)]
#[macro_export]
macro_rules! __output_line {
    () => {
        ::std::format_args!("\n")
    };
    ($($arg:tt)*) => {
        ::std::format_args!("{}\n", ::std::format_args!($($arg)*))
    };
}

/// Prints to stdout like `print!`, without panicking when stdout is closed.
#[macro_export]
macro_rules! out {
    ($($arg:tt)*) => {
        $crate::output::stdout(::std::format_args!($($arg)*))
    };
}

/// Prints to stderr like `eprintln!`, without panicking when stderr is
/// closed. For output addressed to the person running the command;
/// diagnostics are log events.
#[macro_export]
macro_rules! errln {
    ($($arg:tt)*) => {
        $crate::output::stderr($crate::__output_line!($($arg)*))
    };
}

static STDOUT: Stream = Stream::new("stdout");
static STDERR: Stream = Stream::new("stderr");

/// Writes `args` to stdout; see [`outln!`](crate::outln).
pub fn stdout(args: Arguments<'_>) {
    STDOUT.write(&mut io::stdout().lock(), args);
}

/// Writes `args` to stderr; see [`errln!`](crate::errln).
pub fn stderr(args: Arguments<'_>) {
    STDERR.write(&mut io::stderr().lock(), args);
}

/// An output stream of the process and whether it still takes output.
struct Stream {
    name: &'static str,
    failed: AtomicBool,
}

impl Stream {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            failed: AtomicBool::new(false),
        }
    }

    /// Writes `args` to `sink`. The first failure is logged and ends the
    /// writing to this stream: nothing more is attempted.
    fn write(&self, sink: &mut dyn Write, args: Arguments<'_>) {
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        let Err(error) = sink.write_fmt(args) else {
            return;
        };
        self.failed.store(true, Ordering::Relaxed);
        self.log_failure(&error);
    }

    /// A reader that went away is the reader's choice, anything else a
    /// fault worth a warning.
    fn log_failure(&self, error: &io::Error) {
        if error.kind() == io::ErrorKind::BrokenPipe {
            tracing::debug!(
                name: "cli.output.closed",
                { cli.output.stream = self.name },
                "the reader of the command's output went away; no more is written"
            );
            return;
        }
        tracing::warn!(
            name: "cli.output.write_failed",
            {
                cli.output.stream = self.name,
                error.type = "io",
                error.message = %err(error),
            },
            "command output could not be written; no more is written"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use tumult_otel::logging::testing::LogCapture;

    use super::Stream;

    /// Takes `good` writes, then fails every one with `kind`.
    struct Failing {
        good: usize,
        kind: io::ErrorKind,
        written: Vec<u8>,
        attempts: usize,
    }

    impl Failing {
        fn after(good: usize, kind: io::ErrorKind) -> Self {
            Self {
                good,
                kind,
                written: Vec::new(),
                attempts: 0,
            }
        }
    }

    impl Write for Failing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.attempts > self.good {
                return Err(self.kind.into());
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_stream_is_given_up_on_without_a_panic() {
        let logs = LogCapture::start(tracing::Level::DEBUG);
        let stream = Stream::new("stdout");
        let mut sink = Failing::after(1, io::ErrorKind::BrokenPipe);

        stream.write(&mut sink, format_args!("first\n"));
        stream.write(&mut sink, format_args!("second\n"));
        stream.write(&mut sink, format_args!("third\n"));

        assert_eq!(sink.written, b"first\n");
        assert_eq!(sink.attempts, 2, "nothing is tried after the failure");
        let logged = logs.snapshot();
        let closed = logged.named("cli.output.closed");
        assert_eq!(closed.len(), 1, "{}", logged.raw);
        assert_eq!(closed[0]["level"], "DEBUG");
        assert_eq!(closed[0]["cli.output.stream"], "stdout");
    }

    #[test]
    fn any_other_failure_is_a_warning_and_ends_the_output_too() {
        let logs = LogCapture::start(tracing::Level::DEBUG);
        let stream = Stream::new("stderr");
        let mut sink = Failing::after(0, io::ErrorKind::StorageFull);

        stream.write(&mut sink, format_args!("first\n"));
        stream.write(&mut sink, format_args!("second\n"));

        assert_eq!(sink.attempts, 1);
        let logged = logs.snapshot();
        let failed = logged.named("cli.output.write_failed");
        assert_eq!(failed.len(), 1, "{}", logged.raw);
        assert_eq!(failed[0]["level"], "WARN");
        assert_eq!(failed[0]["cli.output.stream"], "stderr");
        assert_eq!(failed[0]["error.type"], "io");
        assert!(failed[0]["error.message"].is_string(), "{}", failed[0]);
    }

    #[test]
    fn a_line_is_formatted_like_println_formats_it() {
        let name = "x";
        let stream = Stream::new("stdout");
        let mut sink = Vec::new();

        stream.write(&mut sink, crate::__output_line!());
        stream.write(&mut sink, crate::__output_line!("plain"));
        stream.write(&mut sink, crate::__output_line!("{name} {} {:>3}", 1, 2));

        assert_eq!(String::from_utf8(sink).unwrap(), "\nplain\nx 1   2\n");
    }
}
