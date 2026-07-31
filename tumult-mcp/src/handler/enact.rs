//! Server-wide enactment lock: at most one fault-injection enactment runs
//! at a time across the whole MCP server.
//!
//! The autopilot gate's `ambient.no_concurrent_experiment` rule can only
//! veto what it can see. This lock is the ledger behind that rule: the
//! enactment path takes the slot (and its gate evaluation reads
//! `concurrent_experiments: 0` — it *is* the one allowed enactment), while
//! every concurrent evaluation observes [`EnactLock::in_flight`] as 1 and
//! vetoes. The guard is released on completion AND on error (RAII), so a
//! failed run never wedges the ledger.
//!
//! The lock covers every enact path routed through dispatch:
//! `tumult_autopilot_run` with `execute=true`, `tumult_autopilot_respond`
//! with `approve=true`, `tumult_run_experiment`, and `tumult_gameday_run`.

use std::sync::atomic::{AtomicU32, Ordering};

/// The enactment ledger: a single-slot mutex plus a readable in-flight
/// count for gate evaluations that do not (and must not) take the slot.
pub(crate) struct EnactLock {
    slot: tokio::sync::Mutex<()>,
    in_flight: AtomicU32,
}

impl EnactLock {
    /// Create an empty ledger with the slot free and no enactments in flight.
    pub(crate) fn new() -> Self {
        Self {
            slot: tokio::sync::Mutex::new(()),
            in_flight: AtomicU32::new(0),
        }
    }

    /// Enactments currently in flight (0 or 1). Gate evaluations that do
    /// not hold the slot feed this to the `no_concurrent_experiment` rule.
    pub(crate) fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Try to become the running enactment. `None` while another enactment
    /// holds the slot — the caller must then evaluate its gate with
    /// `concurrent_experiments = self.in_flight()` (i.e. veto), never wait
    /// for the slot: a queued fault is a stale fault.
    pub(crate) fn try_acquire(&self) -> Option<EnactGuard<'_>> {
        let slot = self.slot.try_lock().ok()?;
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        Some(EnactGuard {
            _slot: slot,
            in_flight: &self.in_flight,
        })
    }
}

/// RAII hold on the enactment slot; dropping releases the ledger entry.
pub(crate) struct EnactGuard<'a> {
    _slot: tokio::sync::MutexGuard<'a, ()>,
    in_flight: &'a AtomicU32,
}

impl Drop for EnactGuard<'_> {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_enactment_holds_the_slot() {
        let lock = EnactLock::new();
        assert_eq!(lock.in_flight(), 0);
        let guard = lock.try_acquire().expect("first acquire succeeds");
        assert_eq!(lock.in_flight(), 1);
        assert!(
            lock.try_acquire().is_none(),
            "a second enactment must be refused while one is in flight"
        );
        assert_eq!(lock.in_flight(), 1);
        drop(guard);
        assert_eq!(lock.in_flight(), 0, "drop releases the ledger entry");
        assert!(lock.try_acquire().is_some(), "slot is free after release");
    }
}
