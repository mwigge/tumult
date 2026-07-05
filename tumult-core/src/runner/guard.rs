//! Auto-halt guard monitor: a background thread that evaluates each declared
//! guard on the sampling interval and cancels the method the moment a guard
//! breaches its safe condition `min_breaches` times in a row.

use std::sync::{mpsc, Arc};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::types::{Guard, HaltRecord, Tolerance};

use super::activity::probe_outcome_ok;
use super::telemetry::epoch_nanos_now;
use super::ActivityExecutor;

/// Handle to the background thread that evaluates auto-halt guards while the
/// method runs.
pub(super) struct GuardMonitor {
    /// Dropped to signal the monitor thread to stop (no breach occurred).
    stop_tx: mpsc::Sender<()>,
    /// Joins to the halt record if a guard breached, or `None` otherwise.
    handle: std::thread::JoinHandle<Option<HaltRecord>>,
}

/// Spawn the auto-halt guard monitor thread, if the experiment declares any
/// guards. The monitor evaluates every guard on `sampling.interval`; the
/// moment a guard's safe-condition tolerance is breached `min_breaches` times
/// in a row it records the breach, cancels `method_token` (stopping the
/// method), and exits.
pub(super) fn spawn_guard_monitor(
    experiment: &crate::types::Experiment,
    executor: &Arc<dyn ActivityExecutor>,
    sampling: &super::SamplingConfig,
    method_token: &CancellationToken,
    method_started: Instant,
) -> Option<GuardMonitor> {
    if experiment.guards.is_empty() {
        return None;
    }

    let guards: Vec<Guard> = experiment.guards.clone();
    let executor = Arc::clone(executor);
    let interval = sampling.interval;
    let token = method_token.clone();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let handle = std::thread::spawn(move || {
        run_guard_monitor(
            &guards,
            executor.as_ref(),
            interval,
            &token,
            method_started,
            &stop_rx,
        )
    });

    Some(GuardMonitor { stop_tx, handle })
}

/// Guard evaluation loop. Returns `Some(HaltRecord)` on the first guard that
/// breaches its safe condition `min_breaches` times consecutively, or `None`
/// when the method finishes first (the runner drops `stop_tx`).
fn run_guard_monitor(
    guards: &[Guard],
    executor: &dyn ActivityExecutor,
    interval: std::time::Duration,
    method_token: &CancellationToken,
    method_started: Instant,
    stop_rx: &mpsc::Receiver<()>,
) -> Option<HaltRecord> {
    let mut consecutive = vec![0u32; guards.len()];
    loop {
        for (idx, guard) in guards.iter().enumerate() {
            let outcome = executor.execute(&guard.probe);
            let safe = probe_outcome_ok(&guard.probe, outcome.success, outcome.output.as_deref());
            if safe {
                consecutive[idx] = 0;
                continue;
            }
            consecutive[idx] += 1;
            if consecutive[idx] >= guard.min_breaches {
                // Method durations never exceed u64::MAX milliseconds.
                #[allow(clippy::cast_possible_truncation)]
                let time_to_halt_ms = method_started.elapsed().as_millis() as u64;
                let record = HaltRecord {
                    guard_name: guard.name.clone(),
                    observed: outcome.output,
                    safe_condition: describe_safe_condition(guard.probe.tolerance.as_ref()),
                    breach_count: consecutive[idx],
                    breached_at_ns: epoch_nanos_now(),
                    time_to_halt_ms,
                    // Filled in by the runner after rollbacks complete.
                    rollback_ms: 0,
                };
                // Pull the plug: cancel the method so remaining activities are
                // skipped.
                method_token.cancel();
                return Some(record);
            }
        }

        // The receive timeout doubles as the inter-sample pause: it returns
        // early (`Disconnected`) the instant the runner drops the stop sender
        // when the method completes, so no guard latency is added.
        match stop_rx.recv_timeout(interval) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Stop the guard monitor and return its halt record (if any). A panicked
/// monitor thread is logged and treated as "no halt".
pub(super) fn finish_guard_monitor(monitor: GuardMonitor) -> Option<HaltRecord> {
    let GuardMonitor { stop_tx, handle } = monitor;
    // Dropping the sender disconnects the monitor's receiver, waking it from
    // its inter-sample wait so it exits promptly when no guard breached.
    drop(stop_tx);
    match handle.join() {
        Ok(record) => record,
        Err(_panic) => {
            tracing::warn!("auto-halt guard monitor thread panicked; treating as no halt");
            None
        }
    }
}

/// Human-readable description of a guard's *safe* condition, for the journal
/// and CLI output (e.g. `range [0, 0.05]`).
fn describe_safe_condition(tolerance: Option<&Tolerance>) -> String {
    match tolerance {
        Some(Tolerance::Range { from, to }) => format!("range [{from}, {to}]"),
        Some(Tolerance::Exact { value }) => format!("exact {value}"),
        Some(Tolerance::Regex { pattern }) => format!("regex /{pattern}/"),
        // Guards are validated to carry a tolerance; this is a defensive
        // fallback only.
        None => "probe success".to_string(),
    }
}
