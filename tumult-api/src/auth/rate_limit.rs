//! Login rate limiting (POST /api/auth/login).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Login rate limiting (POST /api/auth/login)

/// Failed-login attempts allowed per key before throttling kicks in.
const LOGIN_BURST: f64 = 5.0;
/// Bucket refill rate (tokens/second): one fresh attempt every 10 s.
const LOGIN_RPS: f64 = 0.1;
/// Bound on tracked buckets; beyond it the map resets rather than growing
/// without limit under username churn.
const MAX_LOGIN_BUCKETS: usize = 4096;

struct LoginBucket {
    tokens: f64,
    last_refill: Instant,
}

/// Token-bucket limiter for failed logins, keyed by `ip|username` — the same
/// pattern as the MCP server's `RateLimiter`
/// (tumult-mcp/src/handler/rate_limit.rs), but a token is consumed only on a
/// *failed* attempt and a success resets the key, so a legitimate user is
/// never throttled by their own successful logins. Cheap and synchronous:
/// buckets are touched for microseconds per attempt, so a plain `Mutex`
/// suffices.
pub(crate) struct LoginRateLimiter {
    rps: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, LoginBucket>>,
}

impl LoginRateLimiter {
    fn new() -> Self {
        Self::with_params(LOGIN_RPS, LOGIN_BURST)
    }

    /// A limiter with explicit parameters.
    fn with_params(rps: f64, burst: f64) -> Self {
        Self {
            rps,
            burst,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Refill `bucket` up to the burst cap at the configured rate.
    fn refill(&self, bucket: &mut LoginBucket, now: Instant) {
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rps).min(self.burst);
        bucket.last_refill = now;
    }

    /// Whether an attempt for `key` must be refused right now (no token
    /// consumed — [`LoginRateLimiter::penalize`] does that on failure). A
    /// poisoned lock fails open: login stays available and the argon2 cost
    /// still bounds attempts.
    pub(crate) fn throttled(&self, key: &str) -> bool {
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            return false;
        };
        if buckets.len() >= MAX_LOGIN_BUCKETS {
            buckets.clear();
        }
        let bucket = buckets
            .entry(key.to_string())
            .or_insert_with(|| LoginBucket {
                tokens: self.burst,
                last_refill: now,
            });
        self.refill(bucket, now);
        bucket.tokens < 1.0
    }

    /// Consume one token after a failed attempt.
    pub(crate) fn penalize(&self, key: &str) {
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            return;
        };
        if buckets.len() >= MAX_LOGIN_BUCKETS {
            buckets.clear();
        }
        let bucket = buckets
            .entry(key.to_string())
            .or_insert_with(|| LoginBucket {
                tokens: self.burst,
                last_refill: now,
            });
        self.refill(bucket, now);
        bucket.tokens = (bucket.tokens - 1.0).max(0.0);
    }

    /// A successful login clears the key's penalty history.
    pub(crate) fn reset(&self, key: &str) {
        let Ok(mut buckets) = self.buckets.lock() else {
            return;
        };
        buckets.remove(key);
    }
}

/// Process-wide login limiter (one daemon, one API surface; state stays
/// inside tumult-api).
static LOGIN_LIMITER: OnceLock<LoginRateLimiter> = OnceLock::new();

pub(crate) fn login_limiter() -> &'static LoginRateLimiter {
    LOGIN_LIMITER.get_or_init(LoginRateLimiter::new)
}
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_limiter_throttles_after_burst_and_resets_on_success() {
        let l = LoginRateLimiter::with_params(0.001, 3.0);
        assert!(!l.throttled("k"));
        l.penalize("k");
        l.penalize("k");
        assert!(!l.throttled("k"), "one token left");
        l.penalize("k");
        assert!(l.throttled("k"), "bucket exhausted");
        l.reset("k");
        assert!(!l.throttled("k"), "success clears the penalty history");
    }

    #[test]
    fn login_limiter_buckets_are_per_key() {
        let l = LoginRateLimiter::with_params(0.001, 1.0);
        l.penalize("a");
        assert!(l.throttled("a"));
        assert!(!l.throttled("b"), "b has its own bucket");
    }

    #[test]
    fn login_limiter_refills_over_time() {
        let l = LoginRateLimiter::with_params(1000.0, 1.0);
        l.penalize("k");
        assert!(l.throttled("k"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(!l.throttled("k"), "bucket refills at the rps rate");
    }
}
