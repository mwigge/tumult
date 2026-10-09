//! Run-history retention is disabled until reports can query archived evidence.
//! Nonzero policies fail closed; terminal runs and audit trails remain intact.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::IngestWriter;

/// Configured retention days (default zero). Startup validates that only zero
/// is requested; direct sweeps independently refuse a nonzero policy.
#[must_use]
pub fn retention_days_from_env() -> u64 {
    std::env::var("TUMULTD_RUN_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Validate a retention setting before starting any daemon tasks.
///
/// # Errors
/// Rejects nonzero and malformed policies until archived evidence is queryable.
pub fn validate_policy(name: &str, raw: Option<&str>) -> Result<(), String> {
    let raw = raw.unwrap_or("0").trim();
    if raw.is_empty() || raw.parse::<u64>() == Ok(0) {
        return Ok(());
    }
    Err(format!("{name} must be 0: historical queries do not yet read archived snapshots; automatic deletion is disabled"))
}

/// The sweep interval from `TUMULTD_RUN_RETENTION_TICK_S` (default 3600s,
/// minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_RUN_RETENTION_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(3600), Duration::from_secs)
}

/// Spawn the retention sweeper (same shutdown contract as the other daemon
/// background tasks: cancel the token and await before draining the
/// writer).
pub fn spawn_run_retention(
    ingest: IngestWriter,
    tick: Duration,
    retention_days: u64,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = sweep_expired_runs(&ingest, retention_days).await {
                        tracing::warn!(error = %e, "run retention sweep failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("run retention sweeper exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// Keep complete hot history until archive-aware reporting is available.
///
/// # Errors
/// Returns an actionable error for nonzero retention; never deletes records.
pub async fn sweep_expired_runs(_ingest: &IngestWriter, retention_days: u64) -> Result<(), String> {
    validate_policy(
        "TUMULTD_RUN_RETENTION_DAYS",
        Some(&retention_days.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::validate_policy;
    #[test]
    fn policy_is_explicit_and_fail_closed() {
        assert!(validate_policy("retention", None).is_ok());
        assert!(validate_policy("retention", Some("0")).is_ok());
        for value in ["1", "90", "18446744073709551615", "invalid", "-1"] {
            assert!(validate_policy("retention", Some(value))
                .unwrap_err()
                .contains("historical"));
        }
    }
}
