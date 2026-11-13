//! Circuit breaker for the forecasting sidecar.
//!
//! A sidecar that is configured but down would otherwise cost every
//! request its full timeout before the built-in provider steps in. The
//! first failure opens the circuit for a cool-down
//! (`TUMULTD_FORECAST_COOLDOWN_S`, 30 s by default): while it is open the
//! sidecar is skipped and the fallback answers at once. After the
//! cool-down the next request tries the sidecar again; a success closes
//! the circuit, another failure reopens it. A cool-down of zero disables
//! the breaker.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long the sidecar is skipped after a failure, unless configured.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct Open {
    until: Instant,
    error: String,
}

/// Shared open/closed state for one sidecar.
#[derive(Debug)]
pub struct Breaker {
    cooldown: Duration,
    open: Mutex<Option<Open>>,
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
            open: Mutex::new(None),
        }
    }

    /// The configured cool-down.
    #[must_use]
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    /// Why the sidecar must be skipped at `now`, or `None` when it may be
    /// tried. An elapsed cool-down closes the circuit.
    #[must_use]
    pub fn open_reason(&self, now: Instant) -> Option<String> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let state = open.as_ref()?;
        if now >= state.until {
            *open = None;
            return None;
        }
        let left = state.until.duration_since(now).as_secs_f64().ceil();
        Some(format!(
            "sidecar circuit open, retry in {left:.0} s: {}",
            state.error
        ))
    }

    /// The sidecar failed at `now`: skip it for the cool-down.
    pub fn record_failure(&self, now: Instant, error: &str) {
        if self.cooldown.is_zero() {
            return;
        }
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        *open = Some(Open {
            until: now + self.cooldown,
            error: error.to_string(),
        });
    }

    /// The sidecar answered: close the circuit.
    pub fn record_success(&self) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        *open = None;
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

    #[test]
    fn a_new_breaker_is_closed() {
        let breaker = Breaker::default();
        assert_eq!(breaker.cooldown(), DEFAULT_COOLDOWN);
        assert!(breaker.open_reason(Instant::now()).is_none());
    }

    #[test]
    fn a_failure_opens_the_circuit_until_the_cooldown_elapses() {
        let breaker = Breaker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        breaker.record_failure(t0, "sidecar unreachable: refused");
        let reason = breaker.open_reason(t0 + Duration::from_secs(4)).unwrap();
        assert!(reason.contains("circuit open"), "{reason}");
        assert!(reason.contains("retry in 6 s"), "{reason}");
        assert!(reason.contains("sidecar unreachable: refused"), "{reason}");
        assert!(breaker.open_reason(t0 + Duration::from_secs(10)).is_none());
        // Elapsed means closed, also for an earlier clock reading.
        assert!(breaker.open_reason(t0 + Duration::from_secs(4)).is_none());
    }

    #[test]
    fn a_success_closes_the_circuit_and_a_new_failure_reopens_it() {
        let breaker = Breaker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        breaker.record_failure(t0, "first");
        breaker.record_success();
        assert!(breaker.open_reason(t0).is_none());
        breaker.record_failure(t0 + Duration::from_secs(20), "second");
        let reason = breaker.open_reason(t0 + Duration::from_secs(21)).unwrap();
        assert!(reason.contains("second"), "{reason}");
    }

    #[test]
    fn a_zero_cooldown_disables_the_breaker() {
        let breaker = Breaker::new(Duration::ZERO);
        let t0 = Instant::now();
        breaker.record_failure(t0, "down");
        assert!(breaker.open_reason(t0).is_none());
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
