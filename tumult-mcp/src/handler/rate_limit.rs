//! Per-client request rate limiting for the MCP server.
//!
//! A simple token bucket per client key. The key is the MCP session id —
//! the only client identity the rust-mcp-sdk transport exposes to handlers
//! (it does not surface the peer IP at any layer we can hook, so true
//! per-IP limiting is not available; one MCP session maps to one client
//! connection in the Streamable HTTP transport, which makes per-session a
//! close proxy). Requests that carry no session (stdio, or pre-initialize)
//! share one global bucket, so unauthenticated request spam is still
//! bounded.
//!
//! Configured via environment:
//! - `TUMULT_MCP_RATE_LIMIT_RPS` — sustained requests per second per
//!   client (default 20). `0` disables limiting entirely.
//! - `TUMULT_MCP_RATE_LIMIT_BURST` — bucket capacity (default 60).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Environment variable: sustained requests per second per client.
const RPS_ENV: &str = "TUMULT_MCP_RATE_LIMIT_RPS";
/// Environment variable: burst capacity per client.
const BURST_ENV: &str = "TUMULT_MCP_RATE_LIMIT_BURST";

const DEFAULT_RPS: f64 = 20.0;
const DEFAULT_BURST: u32 = 60;

/// Bound on tracked client buckets; beyond it the map resets rather than
/// growing without limit under session churn.
const MAX_BUCKETS: usize = 4096;

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Token-bucket rate limiter. Cheap and synchronous: buckets are touched
/// for microseconds per request, so a plain `Mutex` suffices.
pub(crate) struct RateLimiter {
    rps: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// A limiter with explicit parameters; `rps <= 0` disables limiting.
    #[cfg(test)]
    pub(crate) fn new(rps: f64, burst: u32) -> Self {
        Self {
            rps,
            burst: f64::from(burst),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Build from the environment; unparseable values fall back to the
    /// defaults (with a warning) rather than changing the security posture.
    pub(crate) fn from_env() -> Self {
        let rps = std::env::var(RPS_ENV)
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or_else(|| {
                if std::env::var(RPS_ENV).is_ok() {
                    tracing::warn!("invalid {RPS_ENV}; using default {DEFAULT_RPS}");
                }
                DEFAULT_RPS
            });
        let burst = std::env::var(BURST_ENV)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or_else(|| {
                if std::env::var(BURST_ENV).is_ok() {
                    tracing::warn!("invalid {BURST_ENV}; using default {DEFAULT_BURST}");
                }
                DEFAULT_BURST
            });
        // A zero/negative burst with limiting enabled would refuse every
        // request — clamp to one so a misconfiguration degrades to "slow",
        // not "down".
        let burst = if rps > 0.0 { burst.max(1) } else { burst };
        Self {
            rps,
            burst: f64::from(burst),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether limiting is active (`TUMULT_MCP_RATE_LIMIT_RPS=0` disables).
    pub(crate) fn is_enabled(&self) -> bool {
        self.rps > 0.0
    }

    /// Whether one request from `client` is allowed right now.
    pub(crate) fn check(&self, client: &str) -> bool {
        if !self.is_enabled() {
            return true;
        }
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock must not take the server down; fail open here
            // (the concurrency semaphore still bounds actual work).
            return true;
        };
        if buckets.len() >= MAX_BUCKETS {
            buckets.clear();
        }
        let bucket = buckets.entry(client.to_string()).or_insert_with(|| Bucket {
            tokens: self.burst,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rps).min(self.burst);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_limiter_allows_everything() {
        let limiter = RateLimiter::new(0.0, 0);
        assert!(!limiter.is_enabled());
        for _ in 0..1000 {
            assert!(limiter.check("client"));
        }
    }

    #[test]
    fn burst_is_honored_then_requests_are_refused() {
        let limiter = RateLimiter::new(0.001, 2);
        assert!(limiter.check("client"), "first of burst");
        assert!(limiter.check("client"), "second of burst");
        assert!(
            !limiter.check("client"),
            "burst exhausted — request must be refused"
        );
    }

    #[test]
    fn buckets_are_per_client() {
        let limiter = RateLimiter::new(0.001, 1);
        assert!(limiter.check("a"));
        assert!(!limiter.check("a"), "a's bucket is exhausted");
        assert!(limiter.check("b"), "b has its own bucket");
    }

    #[test]
    fn tokens_refill_over_time() {
        let limiter = RateLimiter::new(1000.0, 1);
        assert!(limiter.check("client"));
        assert!(!limiter.check("client"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(limiter.check("client"), "bucket refills at the rps rate");
    }
}
