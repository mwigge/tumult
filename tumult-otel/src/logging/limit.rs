//! A byte budget for the stderr sink.

use std::cell::Cell;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tracing_subscriber::fmt::MakeWriter;

/// Most bytes of a limit kept back for the marker line and the last words
/// of the process. A `process.fatal` and a `process.panic` line are each
/// capped a little above 8 KiB, so this holds both and the marker.
const RESERVED_BYTES: u64 = 32 * 1024;

/// Renders the marker line for a limit of that many bytes, without the
/// newline.
pub(crate) type Marker = Box<dyn Fn(u64) -> String + Send + Sync>;

thread_local! {
    /// Whether the event being emitted on this thread is one of the last
    /// words of the process; see [`last_words`].
    static LAST_WORDS: Cell<bool> = const { Cell::new(false) };
}

/// Runs `emit` with the events it logs marked as the last words of the
/// process: they are written from the part of the budget that ordinary
/// events cannot spend.
pub(crate) fn last_words<T>(emit: impl FnOnce() -> T) -> T {
    let _lowered_on_drop = Raised::raise();
    emit()
}

/// Lowers the [`LAST_WORDS`] flag again, also when `emit` panics.
struct Raised {
    before: bool,
}

impl Raised {
    fn raise() -> Self {
        // A thread whose locals are already gone logs as an ordinary one.
        let before = LAST_WORDS.try_with(|flag| flag.replace(true));
        Self {
            before: before.unwrap_or(false),
        }
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        // Nothing to lower when the thread's locals are gone.
        let _ = LAST_WORDS.try_with(|flag| flag.set(self.before));
    }
}

fn are_last_words() -> bool {
    LAST_WORDS.try_with(Cell::get).unwrap_or(false)
}

/// A sink that stops taking events once its budget is spent.
///
/// The `fmt` layer hands each rendered event over in one write, so an event
/// is kept or dropped whole. The first ordinary event that does not fit
/// closes the sink for ordinary events for good: a later, shorter one is
/// dropped as well. One marker line is written in its place, so the log
/// says where it stops. Only the last words of the process ([`last_words`])
/// follow the marker, from the part of the budget kept back for them; all
/// of it together never exceeds the limit.
pub(crate) struct Limited<W> {
    inner: W,
    /// `None` for no limit.
    budget: Option<Budget>,
}

impl<W> Limited<W> {
    pub(crate) fn new(inner: W, limit: Option<u64>, marker: Marker) -> Self {
        Self {
            inner,
            budget: limit.map(|limit| Budget::new(limit, marker)),
        }
    }
}

/// What becomes of an event handed to a limited sink.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Write,
    /// Dropped, and the first ordinary event to be: the marker goes in its
    /// place.
    Mark,
    Drop,
}

struct Budget {
    limit: u64,
    /// Bytes ordinary events may still write.
    ordinary: AtomicU64,
    /// Bytes anything may still write.
    total: AtomicU64,
    marked: AtomicBool,
    marker: Marker,
}

impl Budget {
    fn new(limit: u64, marker: Marker) -> Self {
        let reserved = RESERVED_BYTES.min(limit / 2);
        Self {
            limit,
            ordinary: AtomicU64::new(limit - reserved),
            total: AtomicU64::new(limit),
            marked: AtomicBool::new(false),
            marker,
        }
    }

    fn admit(&self, len: usize, last_words: bool) -> Admission {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        if last_words {
            return self.admit_last_words(len);
        }
        if take_or_empty(&self.ordinary, len) && take(&self.total, len) {
            return Admission::Write;
        }
        // The last words may have spent the part ordinary events had left.
        self.ordinary.store(0, Ordering::SeqCst);
        if self.marked.swap(true, Ordering::SeqCst) {
            return Admission::Drop;
        }
        Admission::Mark
    }

    fn admit_last_words(&self, len: u64) -> Admission {
        if take(&self.total, len) {
            return Admission::Write;
        }
        Admission::Drop
    }

    /// The marker line with its newline, or `None` when it does not fit.
    fn marker_line(&self) -> Option<String> {
        let mut line = (self.marker)(self.limit);
        line.push('\n');
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        take(&self.total, len).then_some(line)
    }
}

/// Takes `len` out of `counter` when that much is left. Returns whether it
/// was.
fn take(counter: &AtomicU64, len: u64) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(len)
        })
        .is_ok()
}

/// Takes `len` out of `counter`, or empties it when less is left. Returns
/// whether `len` fitted.
fn take_or_empty(counter: &AtomicU64, len: u64) -> bool {
    let update = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
        Some(left.saturating_sub(len))
    });
    // The closure never declines, so either arm holds the counter as it
    // was before this event.
    let (Ok(before) | Err(before)) = update;
    len <= before
}

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for Limited<W> {
    type Writer = LimitedWriter<'a, W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        LimitedWriter {
            inner: self.inner.make_writer(),
            budget: self.budget.as_ref(),
        }
    }
}

pub(crate) struct LimitedWriter<'a, W> {
    inner: W,
    budget: Option<&'a Budget>,
}

impl<W: io::Write> io::Write for LimitedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(budget) = self.budget else {
            self.inner.write_all(buf)?;
            return Ok(buf.len());
        };
        // An event that is dropped on purpose is reported as written: that
        // keeps the caller from retrying or treating the limit as a broken
        // sink.
        match budget.admit(buf.len(), are_last_words()) {
            Admission::Write => self.inner.write_all(buf)?,
            Admission::Mark => {
                if let Some(line) = budget.marker_line() {
                    self.inner.write_all(line.as_bytes())?;
                }
            }
            Admission::Drop => {}
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{last_words, Admission, Budget, RESERVED_BYTES};

    fn budget(limit: u64) -> Budget {
        Budget::new(limit, Box::new(|limit| format!("limit {limit}")))
    }

    #[test]
    fn half_of_a_small_limit_and_a_fixed_part_of_a_large_one_is_kept_back() {
        let small = budget(100);
        assert_eq!(small.admit(50, false), Admission::Write);
        assert_eq!(small.admit(1, false), Admission::Mark);

        let large = budget(1024 * 1024);
        let ordinary = usize::try_from(1024 * 1024 - RESERVED_BYTES).unwrap();
        assert_eq!(large.admit(ordinary, false), Admission::Write);
        assert_eq!(large.admit(1, false), Admission::Mark);
    }

    #[test]
    fn the_marker_is_written_once_and_counts_against_the_limit() {
        let budget = budget(100);
        assert_eq!(budget.admit(60, false), Admission::Mark);
        assert_eq!(budget.marker_line().as_deref(), Some("limit 100\n"));
        assert_eq!(budget.admit(1, false), Admission::Drop);
        // 100 bytes, less the 10 of the marker.
        assert_eq!(budget.admit(91, true), Admission::Drop);
        assert_eq!(budget.admit(90, true), Admission::Write);
        assert_eq!(budget.admit(1, true), Admission::Drop);
    }

    #[test]
    fn a_marker_that_does_not_fit_is_left_out() {
        let budget = budget(6);
        assert_eq!(budget.admit(5, false), Admission::Mark);
        assert_eq!(budget.marker_line(), None);
    }

    #[test]
    fn last_words_that_spent_the_ordinary_part_close_the_sink() {
        let budget = budget(100);
        assert_eq!(budget.admit(80, true), Admission::Write);
        // 50 were left for ordinary events, but only 20 in all.
        assert_eq!(budget.admit(30, false), Admission::Mark);
        assert_eq!(budget.admit(1, false), Admission::Drop);
    }

    #[test]
    fn the_flag_is_raised_for_the_call_only_and_survives_nesting() {
        assert!(!super::are_last_words());
        let (inner, outer) = last_words(|| {
            let inner = last_words(super::are_last_words);
            (inner, super::are_last_words())
        });
        assert!(inner && outer);
        assert!(!super::are_last_words());
    }
}
