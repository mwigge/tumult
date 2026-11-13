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
//!   A probe that never reports back (its request was dropped) is replaced
//!   after [`PROBE_TIMEOUT`].
//!
//! Every admitted call carries a [`Permit`] stamped with the breaker's
//! generation, which advances on each failure and each probe. A result
//! reported with a stale permit is ignored, so a slow success from before
//! a failure cannot close a circuit that has opened since.
//!
//! A cool-down of zero disables the breaker.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::telemetry;

/// How long the sidecar is skipped after a failure, unless configured.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// How long a half-open circuit waits for its probe before admitting a
/// new one: the client's request timeout plus a margin.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

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
            inner: Mutex::new(Inner {
                state: State::Closed,
                generation: 0,
            }),
        }
    }

    /// Override how long a half-open circuit waits for its probe.
    #[must_use]
    pub fn with_probe_timeout(mut self, probe_timeout: Duration) -> Self {
        self.probe_timeout = probe_timeout;
        self
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
        if permit.generation != inner.generation || inner.state == State::Closed {
            return;
        }
        inner.state = State::Closed;
        telemetry::breaker_transition("closed");
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
        inner.state = State::Open {
            until: now + self.cooldown,
        };
        inner.generation += 1;
        telemetry::breaker_transition("open");
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
}
