//! Capture what the current thread logs, for tests that assert on it.
//!
//! One process-wide subscriber is installed on first use and never removed
//! (a per-test scoped subscriber races with the callsite interest cache
//! when tests run in parallel); it hands each event to the buffer of the
//! thread that emitted it, if that thread is capturing.

use std::cell::RefCell;
use std::io::Write;
use std::sync::{Arc, Mutex, Once};

use tracing_subscriber::fmt::MakeWriter;

thread_local! {
    static CAPTURING: RefCell<Option<Captured>> = const { RefCell::new(None) };
}

/// The captured log text; cloning shares the buffer.
#[derive(Clone, Default)]
pub(crate) struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Writes into the emitting thread's buffer; discards when it has none.
struct ThreadWriter;

impl Write for ThreadWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        CAPTURING.with_borrow(|capturing| {
            if let Some(captured) = capturing {
                captured.0.lock().unwrap().extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadWriter {
    type Writer = ThreadWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ThreadWriter
    }
}

/// Ends the capture when dropped.
pub(crate) struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        CAPTURING.set(None);
    }
}

/// Route this thread's log events into the returned buffer until the guard
/// is dropped.
pub(crate) fn capture() -> (Captured, Guard) {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadWriter)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other global subscriber in this test binary");
    });
    let captured = Captured::default();
    CAPTURING.set(Some(captured.clone()));
    (captured, Guard)
}
