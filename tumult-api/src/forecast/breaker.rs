//! Circuit breaker for the forecasting sidecar.
//!
//! A sidecar that is configured but down would otherwise cost every
//! request its full timeout before the built-in provider steps in. The
//! breaker has three states:
//!
//! * **closed** — every request may call the sidecar.
//! * **open** — a call failed; the sidecar is skipped until the cool-down
//!   (`TUMULTD_FORECAST_COOLDOWN_S`, 30 s by default) has passed and the
//!   fallback answers at once.
//! * **half-open** — the cool-down has passed and exactly one request, the
//!   probe, is let through. Its success closes the circuit, its failure
//!   reopens it; every other request keeps getting the fallback meanwhile.
//!   A probe whose request the sidecar turned down (`rejected`) proves
//!   nothing either way: the next request becomes the probe.
//!   A probe that never reports back (its request was dropped) is replaced
//!   after [`PROBE_TIMEOUT`].
//!
//! A sidecar that answers `busy` is alive, so one such answer opens
//! nothing. Busy every time is another matter: [`DEFAULT_BUSY_LIMIT`] busy
//! answers in a row, or busy answers spanning more than the cool-down,
//! with no success in between, open the circuit like a failure
//! ([`Breaker::record_busy`]). The `Retry-After` of a busy answer holds
//! the series that was asked for back for that long, [`RETRY_AFTER_CAP`]
//! at most ([`Breaker::held_back`]).
//!
//! Every admitted call carries a [`Permit`] stamped with the breaker's
//! generation, which advances on each failure and each probe. A result
//! reported with a stale permit is ignored, so a slow success from before
//! a failure cannot close a circuit that has opened since.
//!
//! A cool-down of zero disables the breaker.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::telemetry;

/// How long the sidecar is skipped after a failure, unless configured.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// How long a half-open circuit waits for its probe before admitting a
/// new one: the client's request timeout plus a margin.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Busy answers in a row, with no success between them, that open the
/// circuit.
pub const DEFAULT_BUSY_LIMIT: u32 = 8;

/// The longest a `Retry-After` hint holds a series back.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// Series held back at once, at most.
const MAX_HELD: usize = 1_024;

/// The shortest time between two "every slot is taken" events.
pub const SLOT_EVENT_EVERY: Duration = Duration::from_secs(10);

/// Where the breaker's callers read the time. The daemon's is the
/// monotonic clock; a test hands in its own.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> Instant + Send + Sync>);

impl Clock {
    /// A clock that reads `read`.
    pub fn new(read: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new(Instant::now)
    }
}

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clock")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open { until: Instant },
    HalfOpen { since: Instant },
}

#[derive(Debug)]
struct Inner {
    state: State,
    generation: u64,
    /// Busy answers since the last success or failure, and when the
    /// first of them came.
    busy_run: u32,
    busy_since: Option<Instant>,
    /// Series asked to stay away, each until its instant.
    held: HashMap<String, Instant>,
    /// The last "every slot is taken" event, and the requests that have
    /// found no slot since without one.
    slot_event: Option<Instant>,
    slot_suppressed: u64,
}

impl Inner {
    /// Skip the sidecar until `until`; permits issued so far are stale.
    fn open(&mut self, until: Instant) {
        self.state = State::Open { until };
        self.generation += 1;
        telemetry::breaker_transition("open");
    }

    fn forget_busy(&mut self) {
        self.busy_run = 0;
        self.busy_since = None;
    }

    /// Keep `series` away for `wait` from `now`. The map is bounded: when
    /// it is full the hints that have passed go, and if none has, this
    /// one is not kept.
    fn hold(&mut self, series: &str, now: Instant, wait: Duration) {
        if wait.is_zero() {
            return;
        }
        if self.held.len() >= MAX_HELD {
            self.held.retain(|_, until| now < *until);
        }
        if self.held.len() >= MAX_HELD && !self.held.contains_key(series) {
            return;
        }
        self.held.insert(series.to_string(), now + wait);
    }
}

/// Leave to call the sidecar once; hand it back with the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permit {
    generation: u64,
}

/// Shared state for one sidecar.
#[derive(Debug)]
pub struct Breaker {
    cooldown: Duration,
    probe_timeout: Duration,
    busy_limit: u32,
    clock: Clock,
    inner: Mutex<Inner>,
}

impl Default for Breaker {
    fn default() -> Self {
        Self::new(DEFAULT_COOLDOWN)
    }
}

impl Breaker {
    /// A closed breaker that opens for `cooldown` after a failure.
    #[must_use]
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            probe_timeout: PROBE_TIMEOUT,
            busy_limit: DEFAULT_BUSY_LIMIT,
            clock: Clock::default(),
            inner: Mutex::new(Inner {
                state: State::Closed,
                generation: 0,
                busy_run: 0,
                busy_since: None,
                held: HashMap::new(),
                slot_event: None,
                slot_suppressed: 0,
            }),
        }
    }

    /// Override how long a half-open circuit waits for its probe.
    #[must_use]
    pub fn with_probe_timeout(mut self, probe_timeout: Duration) -> Self {
        self.probe_timeout = probe_timeout;
        self
    }

    /// Override how many busy answers in a row open the circuit.
    #[must_use]
    pub fn with_busy_limit(mut self, busy_limit: u32) -> Self {
        self.busy_limit = busy_limit;
        self
    }

    /// Read the time from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The time, as the breaker's clock has it.
    #[must_use]
    pub fn now(&self) -> Instant {
        (self.clock.0)()
    }

    /// The configured cool-down.
    #[must_use]
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a request arriving at `now` would be turned away. Changes
    /// nothing: an elapsed cool-down still needs [`Breaker::admit`] to
    /// start the probe.
    #[must_use]
    pub fn is_open(&self, now: Instant) -> bool {
        match self.lock().state {
            State::Closed => false,
            State::Open { until } => now < until,
            State::HalfOpen { since } => now.duration_since(since) < self.probe_timeout,
        }
    }

    /// Ask to call the sidecar at `now`. `None` while the circuit is open
    /// or a probe is under way.
    #[must_use]
    pub fn admit(&self, now: Instant) -> Option<Permit> {
        let mut inner = self.lock();
        match inner.state {
            State::Closed => {}
            State::Open { until } if now < until => return None,
            State::HalfOpen { since } if now.duration_since(since) < self.probe_timeout => {
                return None
            }
            State::Open { .. } | State::HalfOpen { .. } => {
                inner.state = State::HalfOpen { since: now };
                inner.generation += 1;
                telemetry::breaker_transition("half_open");
            }
        }
        Some(Permit {
            generation: inner.generation,
        })
    }

    /// The call made under `permit` reached a working sidecar: close the
    /// circuit, unless the circuit has moved on since the permit was
    /// issued.
    pub fn record_success(&self, permit: Permit) {
        let mut inner = self.lock();
        if permit.generation != inner.generation {
            return;
        }
        inner.forget_busy();
        if inner.state == State::Closed {
            return;
        }
        inner.state = State::Closed;
        telemetry::breaker_transition("closed");
    }

    /// The sidecar answered the call made under `permit` at `now` by
    /// turning that one request down. It is alive, which is not the same
    /// as able to forecast: a closed circuit stays closed, and a half-open
    /// one hands the probe to the next request instead of closing. The
    /// hand-off is an open circuit whose cool-down is already over, and is
    /// counted as one.
    pub fn record_rejection(&self, permit: Permit, now: Instant) {
        let mut inner = self.lock();
        if permit.generation != inner.generation {
            return;
        }
        if !matches!(inner.state, State::HalfOpen { .. }) {
            return;
        }
        inner.open(now);
    }

    /// Whether `series` was asked at `now` to stay away a little longer:
    /// its last busy answer came with a `Retry-After` that has not passed.
    #[must_use]
    pub fn held_back(&self, series: &str, now: Instant) -> bool {
        let inner = self.lock();
        inner.held.get(series).is_some_and(|until| now < *until)
    }

    /// The sidecar had no slot for the call made under `permit` at `now`.
    ///
    /// Once is this request's bad luck, and a half-open probe is handed
    /// on as after a rejection. A sidecar that is busy every time is not
    /// working, though: the busy answers since the last success are
    /// counted, and the one that reaches the limit, or comes more than a
    /// cool-down after the first, opens the circuit like a failure.
    ///
    /// `retry_after` is the sidecar's own hint. It holds `series` back for
    /// as long, [`RETRY_AFTER_CAP`] at most, whatever the permit says.
    pub fn record_busy(
        &self,
        permit: Permit,
        now: Instant,
        series: &str,
        retry_after: Option<Duration>,
    ) {
        let mut inner = self.lock();
        if let Some(hint) = retry_after {
            inner.hold(series, now, hint.min(RETRY_AFTER_CAP));
        }
        if permit.generation != inner.generation {
            return;
        }
        inner.busy_run = inner.busy_run.saturating_add(1);
        let since = *inner.busy_since.get_or_insert(now);
        let stuck = inner.busy_run >= self.busy_limit || now.duration_since(since) > self.cooldown;
        if stuck && !self.cooldown.is_zero() {
            inner.forget_busy();
            inner.open(now + self.cooldown);
            return;
        }
        if matches!(inner.state, State::HalfOpen { .. }) {
            inner.open(now);
        }
    }

    /// A request found every slot taken at `now`. `Some(n)` when this one
    /// is to be logged, `n` being the requests since the last event that
    /// were not; `None` within [`SLOT_EVENT_EVERY`] of that event.
    #[must_use]
    pub fn slots_exhausted(&self, now: Instant) -> Option<u64> {
        let mut inner = self.lock();
        let recent = |last: Instant| now.duration_since(last) < SLOT_EVENT_EVERY;
        if inner.slot_event.is_some_and(recent) {
            inner.slot_suppressed += 1;
            return None;
        }
        inner.slot_event = Some(now);
        Some(std::mem::take(&mut inner.slot_suppressed))
    }

    /// The call made under `permit` failed at `now`: skip the sidecar for
    /// the cool-down, unless the circuit has moved on since the permit was
    /// issued.
    pub fn record_failure(&self, permit: Permit, now: Instant) {
        if self.cooldown.is_zero() {
            return;
        }
        let mut inner = self.lock();
        if permit.generation != inner.generation {
            return;
        }
        inner.forget_busy();
        inner.open(now + self.cooldown);
    }
}

/// `TUMULTD_FORECAST_COOLDOWN_S` → cool-down. Unset or blank means the
/// default; `0` disables the breaker.
///
/// # Errors
/// Returns the offending text when it is not a whole number of seconds.
pub fn parse_cooldown(raw: Option<&str>) -> Result<Duration, String> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(DEFAULT_COOLDOWN);
    };
    text.parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|_| format!("{text:?} is not a whole number of seconds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEN: Duration = Duration::from_secs(10);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_new_breaker_is_closed() {
        let breaker = Breaker::default();
        assert_eq!(breaker.cooldown(), DEFAULT_COOLDOWN);
        let now = Instant::now();
        assert!(!breaker.is_open(now));
        assert!(breaker.admit(now).is_some());
        assert!(breaker.admit(now).is_some(), "closed admits everyone");
    }

    #[test]
    fn a_failure_opens_the_circuit_until_the_cooldown_elapses() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let permit = breaker.admit(t0).unwrap();
        breaker.record_failure(permit, t0);
        assert!(breaker.is_open(t0 + secs(4)));
        assert!(breaker.admit(t0 + secs(4)).is_none());
        assert!(!breaker.is_open(t0 + secs(10)), "the probe may go");
        // Looking does not start the probe: an earlier reading is still open.
        assert!(breaker.is_open(t0 + secs(4)));
    }

    #[test]
    fn half_open_admits_exactly_one_probe() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let after = t0 + secs(11);
        let probe = breaker.admit(after);
        assert!(probe.is_some(), "the cool-down has passed");
        assert!(breaker.admit(after).is_none(), "one probe at a time");
        assert!(breaker.admit(after + secs(1)).is_none());
        assert!(breaker.is_open(after + secs(1)));

        breaker.record_success(probe.unwrap());
        assert!(!breaker.is_open(after + secs(1)));
        assert!(breaker.admit(after + secs(1)).is_some());
        assert!(breaker.admit(after + secs(1)).is_some());
    }

    #[test]
    fn a_failed_probe_reopens_the_circuit_for_a_full_cooldown() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let probe = breaker.admit(t0 + secs(11)).unwrap();
        breaker.record_failure(probe, t0 + secs(12));
        assert!(breaker.admit(t0 + secs(21)).is_none());
        assert!(breaker.admit(t0 + secs(22)).is_some());
    }

    #[test]
    fn a_probe_that_never_reports_is_replaced_after_its_timeout() {
        let breaker = Breaker::new(TEN).with_probe_timeout(secs(5));
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let lost = breaker.admit(t0 + secs(10)).unwrap();
        assert!(breaker.admit(t0 + secs(14)).is_none());
        let replacement = breaker.admit(t0 + secs(15)).unwrap();
        assert_ne!(lost, replacement);
        // The lost probe turning up late decides nothing.
        breaker.record_success(lost);
        assert!(breaker.is_open(t0 + secs(16)));
        breaker.record_success(replacement);
        assert!(!breaker.is_open(t0 + secs(16)));
    }

    /// Two calls leave together; the first fails and opens the circuit.
    /// The second succeeding late must not close it again.
    #[test]
    fn a_late_success_cannot_close_a_freshly_opened_circuit() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let slow = breaker.admit(t0).unwrap();
        let failing = breaker.admit(t0).unwrap();
        breaker.record_failure(failing, t0 + secs(1));
        breaker.record_success(slow);
        assert!(breaker.is_open(t0 + secs(2)), "still open");
        assert!(breaker.admit(t0 + secs(2)).is_none());
        // Nor does a late failure push the cool-down out.
        breaker.record_failure(slow, t0 + secs(9));
        assert!(breaker.admit(t0 + secs(11)).is_some());
    }

    #[test]
    fn a_success_while_closed_changes_nothing() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let permit = breaker.admit(t0).unwrap();
        breaker.record_success(permit);
        assert_eq!(breaker.admit(t0).unwrap(), permit, "same generation");
    }

    #[test]
    fn a_zero_cooldown_disables_the_breaker() {
        let breaker = Breaker::new(Duration::ZERO);
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        assert!(!breaker.is_open(t0));
        assert!(breaker.admit(t0).is_some());
    }

    #[test]
    fn the_cooldown_parses_from_whole_seconds() {
        assert_eq!(parse_cooldown(None), Ok(DEFAULT_COOLDOWN));
        assert_eq!(parse_cooldown(Some("  ")), Ok(DEFAULT_COOLDOWN));
        assert_eq!(parse_cooldown(Some("5")), Ok(Duration::from_secs(5)));
        assert_eq!(parse_cooldown(Some(" 0 ")), Ok(Duration::ZERO));
        assert!(parse_cooldown(Some("soon")).is_err());
        assert!(parse_cooldown(Some("-1")).is_err());
        assert!(parse_cooldown(Some("1.5")).is_err());
    }

    /// The sidecar turned the probe's request down: that says it is alive
    /// but not that it can forecast, so the circuit neither closes nor
    /// cools down again. The next request is the new probe.
    #[test]
    fn a_rejected_probe_hands_the_probe_to_the_next_request() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let after = t0 + secs(11);
        let probe = breaker.admit(after).unwrap();
        breaker.record_rejection(probe, after);
        let next = breaker.admit(after);
        assert!(next.is_some(), "no second cool-down");
        assert!(breaker.admit(after).is_none(), "still one probe at a time");
        breaker.record_success(next.unwrap());
        assert!(breaker.admit(after).is_some());
        assert!(breaker.admit(after).is_some(), "closed by a real answer");
    }

    /// Each change of state is counted once, the hand-off of a rejected
    /// probe included: without it the counter shows two probes in a row
    /// and nothing between them.
    #[test]
    fn every_transition_is_counted_the_rejection_hand_off_too() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let _ = telemetry::take_transitions();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let after = t0 + secs(11);
        let probe = breaker.admit(after).unwrap();
        assert_eq!(telemetry::take_transitions(), ["open", "half_open"]);
        breaker.record_rejection(probe, after);
        assert_eq!(telemetry::take_transitions(), ["open"]);
        breaker.record_success(breaker.admit(after).unwrap());
        assert_eq!(telemetry::take_transitions(), ["half_open", "closed"]);
        // A rejection that changes nothing is not a transition.
        breaker.record_rejection(breaker.admit(after).unwrap(), after);
        breaker.record_rejection(probe, after);
        assert_eq!(telemetry::take_transitions(), [""; 0]);
    }

    /// `n` busy answers, one a second from `from`; the time of the last.
    fn busy_run(breaker: &Breaker, from: Instant, n: u64) -> Instant {
        for i in 0..n {
            let at = from + secs(i);
            let permit = breaker.admit(at).expect("admitted until the run is over");
            breaker.record_busy(permit, at, "s", None);
        }
        from + secs(n.saturating_sub(1))
    }

    /// A sidecar that is busy every time is no better than one that is
    /// down: the eighth busy answer in a row opens the circuit for a full
    /// cool-down.
    #[test]
    fn busy_answers_in_a_row_open_the_circuit_at_the_limit() {
        let breaker = Breaker::new(secs(600));
        let t0 = Instant::now();
        let seventh = busy_run(&breaker, t0, 7);
        assert!(!breaker.is_open(seventh), "seven is not yet the limit");
        let eighth = seventh + secs(1);
        breaker.record_busy(breaker.admit(eighth).unwrap(), eighth, "s", None);
        assert!(breaker.admit(eighth).is_none(), "the eighth opens");
        assert!(
            breaker.admit(eighth + secs(599)).is_none(),
            "a full cool-down"
        );
        assert!(
            breaker.admit(eighth + secs(600)).is_some(),
            "then one probe"
        );
    }

    #[test]
    fn the_busy_limit_can_be_set() {
        let breaker = Breaker::new(secs(600)).with_busy_limit(2);
        let t0 = Instant::now();
        let second = busy_run(&breaker, t0, 2);
        assert!(breaker.admit(second).is_none());
    }

    /// One real answer shows the sidecar still gets work done: the count
    /// starts again.
    #[test]
    fn a_success_between_busy_answers_starts_the_count_again() {
        let breaker = Breaker::new(secs(600));
        let t0 = Instant::now();
        let last = busy_run(&breaker, t0, 7);
        breaker.record_success(breaker.admit(last).unwrap());
        let last = busy_run(&breaker, last, 7);
        assert!(!breaker.is_open(last), "seven, a success, seven more");
        breaker.record_busy(breaker.admit(last).unwrap(), last, "s", None);
        assert!(breaker.is_open(last), "the eighth since the success");
    }

    /// Few requests never reach the count. A sidecar that has been busy
    /// for longer than the cool-down, with no success in between, opens
    /// the circuit on time alone.
    #[test]
    fn busy_for_longer_than_the_cooldown_opens_the_circuit() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "s", None);
        let at = t0 + secs(10);
        breaker.record_busy(breaker.admit(at).unwrap(), at, "s", None);
        assert!(!breaker.is_open(at), "exactly the cool-down is not longer");
        let at = t0 + secs(11);
        breaker.record_busy(breaker.admit(at).unwrap(), at, "s", None);
        assert!(
            breaker.admit(at + secs(9)).is_none(),
            "open for a cool-down"
        );
        assert!(breaker.admit(at + secs(10)).is_some());
    }

    /// The time is counted from the first busy answer since a success,
    /// not from the first ever.
    #[test]
    fn a_success_also_restarts_the_busy_time() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "s", None);
        breaker.record_success(breaker.admit(t0 + secs(5)).unwrap());
        let at = t0 + secs(11);
        breaker.record_busy(breaker.admit(at).unwrap(), at, "s", None);
        assert!(!breaker.is_open(at), "busy for no time at all");
        let at = t0 + secs(22);
        breaker.record_busy(breaker.admit(at).unwrap(), at, "s", None);
        assert!(breaker.is_open(at), "eleven seconds of busy");
    }

    /// A busy probe is handed on, and still counts: probes that are busy
    /// every time end in a full cool-down, not in probing for ever.
    #[test]
    fn busy_probes_are_handed_on_until_the_limit_then_cool_down() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let after = t0 + secs(11);
        let _ = telemetry::take_transitions();
        for nth in 1..=7 {
            let probe = breaker.admit(after).expect("handed on");
            breaker.record_busy(probe, after, "s", None);
            assert!(!breaker.is_open(after), "probe {nth} is handed on");
        }
        let eighth = breaker.admit(after).unwrap();
        breaker.record_busy(eighth, after, "s", None);
        assert!(breaker.admit(after).is_none(), "no ninth probe");
        assert!(breaker.admit(after + secs(9)).is_none());
        assert!(breaker.admit(after + secs(10)).is_some());
        let opened = telemetry::take_transitions();
        assert_eq!(opened.iter().filter(|s| **s == "open").count(), 8);
    }

    /// A failure is the start of something else: the busy answers before
    /// it are not carried into the next probe.
    #[test]
    fn a_failure_forgets_the_busy_answers_before_it() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let last = busy_run(&breaker, t0, 7);
        breaker.record_failure(breaker.admit(last).unwrap(), last);
        let after = last + secs(10);
        breaker.record_busy(breaker.admit(after).unwrap(), after, "s", None);
        assert!(
            breaker.admit(after).is_some(),
            "handed on: one busy, not eight"
        );
    }

    #[test]
    fn busy_answers_never_open_a_disabled_breaker() {
        let breaker = Breaker::new(Duration::ZERO);
        let t0 = Instant::now();
        let last = busy_run(&breaker, t0, 20);
        assert!(!breaker.is_open(last + secs(1)));
    }

    /// An answer from before the circuit last opened decides nothing.
    #[test]
    fn an_out_of_date_busy_answer_is_not_counted() {
        let breaker = Breaker::new(secs(600)).with_busy_limit(2);
        let t0 = Instant::now();
        let old = breaker.admit(t0).unwrap();
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        let after = t0 + secs(600);
        let probe = breaker.admit(after).unwrap();
        breaker.record_busy(old, after, "s", None);
        breaker.record_busy(old, after, "s", None);
        breaker.record_success(probe);
        assert!(!breaker.is_open(after), "two stale busy answers opened it");
    }

    /// `Retry-After` is a hint for the series that was asked for: that
    /// series stays away for as long, the others are sent as before.
    #[test]
    fn retry_after_holds_that_series_back_and_no_other() {
        let breaker = Breaker::new(secs(600));
        let t0 = Instant::now();
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "a", Some(secs(5)));
        assert!(breaker.held_back("a", t0));
        assert!(breaker.held_back("a", t0 + secs(4)));
        assert!(!breaker.held_back("a", t0 + secs(5)), "the hint has passed");
        assert!(!breaker.held_back("b", t0 + secs(1)), "another series");
        assert!(
            breaker.admit(t0 + secs(1)).is_some(),
            "the circuit is closed"
        );
    }

    /// The sidecar's word is a hint, not an order: a long one is cut.
    #[test]
    fn a_long_retry_after_is_capped() {
        let breaker = Breaker::new(secs(600));
        let t0 = Instant::now();
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "a", Some(secs(86_400)));
        assert!(breaker.held_back("a", t0 + RETRY_AFTER_CAP - secs(1)));
        assert!(!breaker.held_back("a", t0 + RETRY_AFTER_CAP));
    }

    #[test]
    fn a_busy_answer_without_a_hint_holds_nothing_back() {
        let breaker = Breaker::new(secs(600));
        let t0 = Instant::now();
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "a", None);
        breaker.record_busy(breaker.admit(t0).unwrap(), t0, "z", Some(Duration::ZERO));
        assert!(!breaker.held_back("a", t0));
        assert!(!breaker.held_back("z", t0));
    }

    /// The hints are bounded: names that are held back no longer are
    /// dropped to make room, and when every one is still held back the
    /// newcomer is the one that is not remembered.
    #[test]
    fn the_series_held_back_are_bounded() {
        let breaker = Breaker::new(Duration::ZERO);
        let t0 = Instant::now();
        let hold = |name: &str, at: Instant, s: u64| {
            breaker.record_busy(breaker.admit(at).unwrap(), at, name, Some(secs(s)));
        };
        for i in 0..MAX_HELD {
            hold(&format!("short-{i}"), t0, 1);
        }
        hold("one-too-many", t0, 1);
        assert!(!breaker.held_back("one-too-many", t0), "no room");
        assert!(breaker.held_back("short-0", t0));
        assert_eq!(breaker.lock().held.len(), MAX_HELD);
        hold("later", t0 + secs(2), 5);
        assert!(
            breaker.held_back("later", t0 + secs(3)),
            "the old ones went"
        );
        assert_eq!(breaker.lock().held.len(), 1);
    }

    /// Slot exhaustion comes in bursts; one event per burst says how many
    /// requests it stands for.
    #[test]
    fn slot_exhaustion_is_reported_once_per_interval_with_the_rest_counted() {
        let breaker = Breaker::default();
        let t0 = Instant::now();
        assert_eq!(
            breaker.slots_exhausted(t0),
            Some(0),
            "the first is reported"
        );
        assert_eq!(breaker.slots_exhausted(t0), None);
        assert_eq!(breaker.slots_exhausted(t0 + secs(9)), None);
        let next = t0 + SLOT_EVENT_EVERY;
        assert_eq!(breaker.slots_exhausted(next), Some(2), "two were not");
        assert_eq!(breaker.slots_exhausted(next + secs(1)), None);
        assert_eq!(breaker.slots_exhausted(next + secs(11)), Some(1));
    }

    #[test]
    fn the_breaker_reads_the_clock_it_was_given() {
        let fixed = Instant::now() + secs(1_000);
        let breaker = Breaker::default().with_clock(Clock::new(move || fixed));
        assert_eq!(breaker.now(), fixed);
    }

    #[test]
    fn a_rejection_while_closed_or_out_of_date_changes_nothing() {
        let breaker = Breaker::new(TEN);
        let t0 = Instant::now();
        let permit = breaker.admit(t0).unwrap();
        breaker.record_rejection(permit, t0);
        assert_eq!(breaker.admit(t0).unwrap(), permit, "same generation");
        // A rejection from before the circuit opened decides nothing.
        breaker.record_failure(breaker.admit(t0).unwrap(), t0);
        breaker.record_rejection(permit, t0 + secs(1));
        assert!(breaker.admit(t0 + secs(2)).is_none(), "still cooling down");
    }
}
