//! Peer-controlled text on its way into a log line or a status message.
//!
//! Anything a client chooses (schema ids, field names, error text built
//! from them) is bounded and stripped of control characters by [`clean`]
//! before it is logged or sent back, so a peer can neither forge log lines
//! nor make one request write megabytes of log. Cleaned text is logged as
//! a string field, which the log format puts in quotes; [`clean`] escapes
//! the quote, so the text cannot end its value and add `key=value` pairs
//! of its own to the line. Warnings a peer can trigger
//! at will go through a [`LogLimiter`], which lets a burst through and
//! counts the rest, kind by kind: a flood of one warning does not hide the
//! first of another.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use arrow::datatypes::DataType;

/// Bytes [`clean`] lets through.
pub(crate) const MAX_TEXT_BYTES: usize = 256;

/// Appended where [`clean`] cut the text.
const CUT: &str = "...";

/// Warnings of one kind let through per [`WARN_WINDOW`].
pub(crate) const WARN_BURST: u32 = 10;

/// See [`WARN_BURST`].
pub(crate) const WARN_WINDOW: Duration = Duration::from_secs(10);

/// Characters that end or reorder a line in some viewer without being
/// control characters to `char::is_control`.
fn reshapes_a_line(c: char) -> bool {
    matches!(
        c,
        '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{061c}'
            | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
}

/// `text` made safe to log or to send in a status: at most
/// [`MAX_TEXT_BYTES`] bytes, control characters (line breaks, escape
/// sequences), quotes and backslashes escaped.
pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_BYTES));
    let mut piece = String::new();
    for c in text.chars() {
        piece.clear();
        if matches!(c, '\\' | '"') || c.is_control() || reshapes_a_line(c) {
            piece.extend(c.escape_default());
        } else {
            piece.push(c);
        }
        if out.len() + piece.len() > MAX_TEXT_BYTES - CUT.len() {
            out.push_str(CUT);
            break;
        }
        out.push_str(&piece);
    }
    out
}

/// [`clean`] for anything displayable (errors, statuses).
pub(crate) fn clean_display(value: &dyn std::fmt::Display) -> String {
    clean(&value.to_string())
}

/// An Arrow type named by its kind only. The full `Display` repeats what
/// the peer put in it: field names, timezones, metadata.
pub(crate) fn kind(data_type: &DataType) -> &'static str {
    match data_type {
        DataType::Null => "null",
        DataType::Boolean => "bool",
        DataType::Int8 => "int8",
        DataType::Int16 => "int16",
        DataType::Int32 => "int32",
        DataType::Int64 => "int64",
        DataType::UInt8 => "uint8",
        DataType::UInt16 => "uint16",
        DataType::UInt32 => "uint32",
        DataType::UInt64 => "uint64",
        DataType::Float16 => "float16",
        DataType::Float32 => "float32",
        DataType::Float64 => "float64",
        DataType::Timestamp(..) => "timestamp",
        DataType::Date32 | DataType::Date64 => "date",
        DataType::Time32(_) | DataType::Time64(_) => "time",
        DataType::Duration(_) => "duration",
        DataType::Interval(_) => "interval",
        DataType::Binary => "binary",
        DataType::FixedSizeBinary(_) => "fixed-size binary",
        DataType::LargeBinary => "large binary",
        DataType::BinaryView => "binary view",
        DataType::Utf8 => "utf8",
        DataType::LargeUtf8 => "large utf8",
        DataType::Utf8View => "utf8 view",
        DataType::List(_) => "list",
        DataType::ListView(_) => "list view",
        DataType::FixedSizeList(..) => "fixed-size list",
        DataType::LargeList(_) => "large list",
        DataType::LargeListView(_) => "large list view",
        DataType::Struct(_) => "struct",
        DataType::Union(..) => "union",
        DataType::Dictionary(..) => "dictionary",
        DataType::Decimal32(..)
        | DataType::Decimal64(..)
        | DataType::Decimal128(..)
        | DataType::Decimal256(..) => "decimal",
        DataType::Map(..) => "map",
        DataType::RunEndEncoded(..) => "run-end encoded",
    }
}

/// One kind of event under a [`LogLimiter`].
struct Kind {
    name: &'static str,
    /// Whether one was let through in the current window.
    logged: bool,
    /// How many were left out since the last one let through.
    suppressed: u64,
}

struct Window {
    started: Option<Instant>,
    logged: u32,
    /// The kinds seen so far. They are named in the code, never by a
    /// peer, so there are only a few.
    kinds: Vec<Kind>,
}

/// Lets `burst` events through per `window` and counts the others. The
/// first event of each kind in a window is let through even when the
/// burst is used up, so a window logs at most `burst` lines and one more
/// per kind.
pub(crate) struct LogLimiter {
    burst: u32,
    window: Duration,
    state: Mutex<Window>,
}

impl LogLimiter {
    pub(crate) const fn new(burst: u32, window: Duration) -> Self {
        Self {
            burst,
            window,
            state: Mutex::new(Window {
                started: None,
                logged: 0,
                kinds: Vec::new(),
            }),
        }
    }

    /// A limiter with the default burst and window.
    pub(crate) const fn warnings() -> Self {
        Self::new(WARN_BURST, WARN_WINDOW)
    }

    /// `Some(n)` when the event of `kind` may be logged, `n` being how
    /// many of that kind were suppressed since the last one that was;
    /// `None` when it may not.
    pub(crate) fn allow(&self, kind: &'static str) -> Option<u64> {
        self.allow_at(kind, Instant::now())
    }

    fn allow_at(&self, kind: &'static str, now: Instant) -> Option<u64> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let expired = match state.started {
            Some(started) => now.saturating_duration_since(started) >= self.window,
            None => true,
        };
        if expired {
            state.started = Some(now);
            state.logged = 0;
            state.kinds.iter_mut().for_each(|kind| kind.logged = false);
        }
        let in_burst = state.logged < self.burst;
        let position = match state.kinds.iter().position(|known| known.name == kind) {
            Some(position) => position,
            None => {
                state.kinds.push(Kind {
                    name: kind,
                    logged: false,
                    suppressed: 0,
                });
                state.kinds.len() - 1
            }
        };
        let entry = &mut state.kinds[position];
        if !in_burst && entry.logged {
            entry.suppressed = entry.suppressed.saturating_add(1);
            return None;
        }
        entry.logged = true;
        let suppressed = std::mem::take(&mut entry.suppressed);
        if in_burst {
            state.logged += 1;
        }
        Some(suppressed)
    }
}

/// Rejected ingest tokens, across the gRPC and HTTP carriers: anyone who
/// can reach the listener can cause these.
#[cfg(not(test))]
pub(crate) static REJECTED_TOKENS: LogLimiter = LogLimiter::warnings();

/// [`warn_rejected_token`] under the process-wide limiter.
#[cfg(not(test))]
pub(crate) fn warn_rejected(carrier: &'static str, presented: bool, peer: Option<IpAddr>) {
    warn_rejected_token(&REJECTED_TOKENS, carrier, presented, peer);
}

/// [`warn_rejected_token`] under a limiter per test thread, so tests do not
/// use up each other's burst.
#[cfg(test)]
pub(crate) fn warn_rejected(carrier: &'static str, presented: bool, peer: Option<IpAddr>) {
    thread_local! {
        static LIMITER: LogLimiter = const { LogLimiter::warnings() };
    }
    LIMITER.with(|limiter| warn_rejected_token(limiter, carrier, presented, peer));
}

/// Log one rejected bearer token (rate-limited), saying where the call
/// came from and whether a credential was presented, never what it was.
/// Behind a proxy `peer` is the proxy.
pub(crate) fn warn_rejected_token(
    limiter: &LogLimiter,
    carrier: &'static str,
    presented: bool,
    peer: Option<IpAddr>,
) {
    let Some(suppressed) = limiter.allow(carrier) else {
        return;
    };
    tracing::warn!(
        carrier,
        presented,
        peer = peer.map(tracing::field::display),
        suppressed,
        "ingest bearer token rejected"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_escapes_what_could_forge_or_garble_a_log_line() {
        assert_eq!(clean("plain text, ünïcode"), "plain text, ünïcode");
        assert_eq!(clean("a\nINFO forged\r"), "a\\nINFO forged\\r");
        assert_eq!(clean("tab\there"), "tab\\there");
        assert_eq!(clean("esc\u{1b}[31m"), "esc\\u{1b}[31m");
        assert_eq!(clean("nul\0"), "nul\\u{0}");
        assert_eq!(clean("back\\slash"), "back\\\\slash");
        assert_eq!(clean("ls\u{2028}ps\u{2029}"), "ls\\u{2028}ps\\u{2029}");
        assert_eq!(clean("rtl\u{202e}x"), "rtl\\u{202e}x");
        assert_eq!(clean("c1\u{85}"), "c1\\u{85}");
        assert_eq!(clean("alm\u{61c}x"), "alm\\u{61c}x");
        // A quote would end the quoted value the text is logged as.
        assert_eq!(clean("id\" level=\"ERROR"), "id\\\" level=\\\"ERROR");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn clean_cuts_long_text_on_a_character_boundary() {
        let cut = clean(&"x".repeat(10_000));
        assert_eq!(cut.len(), MAX_TEXT_BYTES);
        assert!(cut.ends_with("xxx..."));
        let exact = "y".repeat(MAX_TEXT_BYTES - CUT.len());
        assert_eq!(clean(&exact), exact);
        // Multi-byte characters and escapes never push it over.
        for text in [
            "é".repeat(5000),
            "\n".repeat(5000),
            "\u{1f600}".repeat(5000),
        ] {
            let cut = clean(&text);
            assert!(cut.len() <= MAX_TEXT_BYTES, "{}", cut.len());
            assert!(cut.ends_with(CUT));
            assert!(!cut.chars().any(char::is_control));
        }
    }

    #[test]
    fn kinds_carry_no_peer_text() {
        let zone: std::sync::Arc<str> = "evil\nzone".into();
        let stamp = DataType::Timestamp(arrow::datatypes::TimeUnit::Nanosecond, Some(zone));
        assert_eq!(kind(&stamp), "timestamp");
        let field = arrow::datatypes::Field::new("evil\nname", DataType::Int32, true);
        assert_eq!(kind(&DataType::List(std::sync::Arc::new(field))), "list");
        assert_eq!(
            kind(&DataType::Dictionary(
                Box::new(DataType::UInt8),
                Box::new(DataType::Utf8)
            )),
            "dictionary"
        );
    }

    #[test]
    fn limiter_lets_a_burst_through_and_reports_what_it_suppressed() {
        let limiter = LogLimiter::new(3, Duration::from_secs(10));
        let start = Instant::now();
        let allowed: Vec<Option<u64>> = (0..5).map(|_| limiter.allow_at("a", start)).collect();
        assert_eq!(allowed, [Some(0), Some(0), Some(0), None, None]);
        // Still inside the window.
        assert_eq!(limiter.allow_at("a", start + Duration::from_secs(9)), None);
        // The next window opens with the count of what was dropped.
        let later = start + Duration::from_secs(10);
        assert_eq!(limiter.allow_at("a", later), Some(3));
        assert_eq!(limiter.allow_at("a", later), Some(0));
    }

    #[test]
    fn a_flood_of_one_kind_does_not_hide_the_first_of_another() {
        let limiter = LogLimiter::new(3, Duration::from_secs(10));
        let start = Instant::now();
        let flood: Vec<Option<u64>> = (0..50).map(|_| limiter.allow_at("flood", start)).collect();
        assert_eq!(flood.iter().flatten().count(), 3, "the burst");
        // The burst is used up; the first event of another kind is let
        // through all the same, and only the first.
        assert_eq!(limiter.allow_at("rare", start), Some(0));
        assert_eq!(limiter.allow_at("rare", start), None);
        assert_eq!(limiter.allow_at("rare", start), None);
        assert_eq!(limiter.allow_at("flood", start), None);
        // Each kind reports what was left out of its own.
        let later = start + Duration::from_secs(10);
        assert_eq!(limiter.allow_at("rare", later), Some(2));
        assert_eq!(limiter.allow_at("flood", later), Some(48));
        assert_eq!(limiter.allow_at("flood", later), Some(0));
    }

    #[test]
    fn a_window_logs_at_most_the_burst_and_one_line_per_kind() {
        let limiter = LogLimiter::new(4, Duration::from_secs(10));
        let start = Instant::now();
        let kinds = ["a", "b", "c", "d", "e", "f"];
        let logged = (0..600)
            .filter(|n| limiter.allow_at(kinds[n % kinds.len()], start).is_some())
            .count();
        assert_eq!(
            logged, 6,
            "the burst, then the first of the two kinds after it"
        );
        let logged = (0..600)
            .filter(|_| {
                limiter
                    .allow_at("a", start + Duration::from_secs(10))
                    .is_some()
            })
            .count();
        assert_eq!(logged, 4);
        let others = kinds[1..]
            .iter()
            .filter(|kind| {
                limiter
                    .allow_at(kind, start + Duration::from_secs(11))
                    .is_some()
            })
            .count();
        assert_eq!(
            others, 5,
            "one line each after the burst went to another kind"
        );
    }

    #[test]
    fn rejected_token_warning_is_limited_and_names_the_carrier() {
        let (logs, _guard) = crate::test_logs::capture();
        let limiter = LogLimiter::new(2, Duration::from_secs(3600));
        let peer = IpAddr::from([203, 0, 113, 9]);
        for _ in 0..50 {
            warn_rejected_token(&limiter, "grpc", true, Some(peer));
        }
        let text = logs.text();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("carrier=\"grpc\"") && text.contains("presented=true"));
        assert_eq!(text.matches(" peer=203.0.113.9 ").count(), 2, "{text}");
    }

    #[test]
    fn rejected_token_warning_without_a_known_peer_says_nothing_of_one() {
        let (logs, _guard) = crate::test_logs::capture();
        let limiter = LogLimiter::new(2, Duration::from_secs(3600));
        warn_rejected_token(&limiter, "http", false, None);
        let text = logs.text();
        assert!(text.contains("ingest bearer token rejected"), "{text}");
        assert!(!text.contains("peer"), "{text}");
    }
}
