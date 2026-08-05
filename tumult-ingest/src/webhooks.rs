//! Outbound event notifications: the webhook dispatcher (schema v11
//! `webhooks` / `webhook_cursors`, v13 `webhook_dead_letters`).
//!
//! Mirrors the schedule scheduler: every tick, each enabled webhook receives
//! the `run_audit` events newer than its delivery cursor as signed JSON
//! POSTs. Payloads are signed `X-Tumult-Signature: sha256=<hmac-sha256>`
//! with the webhook's secret.
//!
//! Delivery is bounded-retry with exponential backoff across ticks: a failed
//! event holds the cursor (the batch is retried on a later tick, the
//! endpoint backing off 1/2/4/8 ticks), and after
//! `TUMULTD_WEBHOOK_MAX_ATTEMPTS` (default 5) consecutive failing ticks the
//! pending events are recorded in `webhook_dead_letters` before the cursor
//! advances past them — loss is never silent, and `run_audit` remains the
//! source of truth for replay. Each endpoint dispatches in its own task
//! under a per-endpoint time budget (`TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S`,
//! default 120s), so one dead or hung receiver cannot stall the others, and
//! one endpoint's store error is isolated to that endpoint. Delivery counts
//! (succeeded / failed / dead-lettered) are exported via
//! [`crate::daemon_metrics`].
//!
//! SSRF policy (conservative): URLs must be `https`; plain `http` requires
//! `TUMULTD_WEBHOOK_ALLOW_INSECURE=1`, and loopback / unspecified /
//! private / link-local IP literals require `TUMULTD_WEBHOOK_ALLOW_LOCAL=1`
//! (both exist for local demos and tests). Hostnames are not resolved at
//! validation time — DNS-level rebinding remains an accepted residual risk,
//! mitigated by the Admin-only management role.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::IngestWriter;

mod dispatcher;
mod policy;
pub use dispatcher::Dispatcher;
pub use policy::{hmac_sha256_hex, validate_webhook_url};

/// The dispatcher's tick interval from `TUMULTD_WEBHOOK_TICK_S` (default
/// 15s, minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_WEBHOOK_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(15), Duration::from_secs)
}

/// Consecutive failing dispatch ticks after which an endpoint's pending
/// events are dead-lettered, from `TUMULTD_WEBHOOK_MAX_ATTEMPTS` (default
/// 5, minimum 1); invalid values fall back to the default.
#[must_use]
pub fn max_attempts_from_env() -> u32 {
    std::env::var("TUMULTD_WEBHOOK_MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5)
}

/// One endpoint's per-tick time budget from
/// `TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S` (default 120s, minimum 1s); invalid
/// values fall back to the default. A hung receiver is abandoned at the
/// budget and retried under backoff — other endpoints are unaffected.
#[must_use]
pub fn endpoint_budget_from_env() -> Duration {
    std::env::var("TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(120), Duration::from_secs)
}

/// Spawn the webhook dispatcher (same shutdown contract as the schedule
/// scheduler: cancel the token and await before draining the writer).
pub fn spawn_webhook_dispatcher(
    db_path: PathBuf,
    ingest: IngestWriter,
    tick: Duration,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut dispatcher = match Dispatcher::from_env() {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(error = %e, "webhook dispatcher cannot build its HTTP client; exiting");
                return;
            }
        };
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = dispatcher.dispatch_pending(&db_path, &ingest).await {
                        tracing::warn!(error = %e, "webhook dispatch tick failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("webhook dispatcher exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// One dispatch tick with a throwaway dispatcher (no backoff carried
/// between calls) — the convenience entry point used by callers that do not
/// keep dispatcher state.
///
/// # Errors
/// Returns an error when the store cannot be read at all.
pub async fn dispatch_pending(db_path: &Path, ingest: &IngestWriter) -> Result<usize, String> {
    Dispatcher::from_env()?
        .dispatch_pending(db_path, ingest)
        .await
}
