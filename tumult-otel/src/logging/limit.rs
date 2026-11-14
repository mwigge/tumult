//! A byte budget for the stderr sink.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing_subscriber::fmt::MakeWriter;

/// A sink that stops taking events once its budget is spent.
///
/// The `fmt` layer hands each rendered event over in one write, so an event
/// is kept or dropped whole. The first event that does not fit closes the
/// sink for good: a later, shorter one is dropped as well, so what was
/// written is always a prefix of the log and never has a hole in it.
pub(crate) struct Limited<W> {
    inner: W,
    /// Bytes still to be written; `None` for no limit.
    remaining: Option<AtomicU64>,
}

impl<W> Limited<W> {
    pub(crate) fn new(inner: W, limit: Option<u64>) -> Self {
        Self {
            inner,
            remaining: limit.map(AtomicU64::new),
        }
    }
}

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for Limited<W> {
    type Writer = LimitedWriter<'a, W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        LimitedWriter {
            inner: self.inner.make_writer(),
            remaining: self.remaining.as_ref(),
        }
    }
}

pub(crate) struct LimitedWriter<'a, W> {
    inner: W,
    remaining: Option<&'a AtomicU64>,
}

impl<W> LimitedWriter<'_, W> {
    /// Takes `len` bytes out of the budget, or empties it when they do not
    /// fit. Returns whether they fitted.
    fn admit(&self, len: usize) -> bool {
        let Some(remaining) = self.remaining else {
            return true;
        };
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        let update = remaining.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            Some(left.saturating_sub(len))
        });
        // The closure never declines, so either arm holds the budget as it
        // was before this event.
        let (Ok(before) | Err(before)) = update;
        len <= before
    }
}

impl<W: io::Write> io::Write for LimitedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.admit(buf.len()) {
            // Dropped on purpose: reporting it as written keeps the caller
            // from retrying or treating the limit as a broken sink.
            return Ok(buf.len());
        }
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
