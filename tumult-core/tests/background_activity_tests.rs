//! Integration tests for background activity execution.
//!
//! Verifies:
//! - Background activities run concurrently with foreground activities
//! - `pause_before_s` delays activity start by at least the specified duration
//! - `pause_after_s` delays the start of the next activity by at least the specified duration
//! - A panicking background thread produces a `Failed` `ActivityResult`

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;
use tumult_test_utils::{action, background_action, minimal_experiment};

// ── Helper executor types ─────────────────────────────────────

/// Records the wall-clock instant at which each activity starts executing.
struct TimestampExecutor {
    timestamps: Arc<Mutex<HashMap<String, Instant>>>,
}

impl TimestampExecutor {
    fn new() -> Self {
        Self {
            timestamps: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn timestamps(&self) -> Arc<Mutex<HashMap<String, Instant>>> {
        Arc::clone(&self.timestamps)
    }
}

impl ActivityExecutor for TimestampExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.timestamps
            .lock()
            .unwrap()
            .insert(activity.name.clone(), Instant::now());

        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 0,
        }
    }
}

/// The activity named `waiter` does not finish before the one named
/// `awaited` has started, or a deadline has passed. `overlapped` says which.
struct OverlapExecutor {
    waiter: &'static str,
    awaited: &'static str,
    awaited_started: Mutex<bool>,
    changed: Condvar,
    overlapped: Arc<AtomicBool>,
}

impl OverlapExecutor {
    fn new(waiter: &'static str, awaited: &'static str) -> Self {
        Self {
            waiter,
            awaited,
            awaited_started: Mutex::new(false),
            changed: Condvar::new(),
            overlapped: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ActivityExecutor for OverlapExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        if activity.name == self.awaited {
            *self.awaited_started.lock().unwrap() = true;
            self.changed.notify_all();
        }
        if activity.name == self.waiter {
            let started = self.awaited_started.lock().unwrap();
            let (started, _) = self
                .changed
                .wait_timeout_while(started, Duration::from_secs(20), |started| !*started)
                .unwrap();
            self.overlapped.store(*started, Ordering::SeqCst);
        }
        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 0,
        }
    }
}

/// Executor that panics for a specific activity name.
struct PanicOnNameExecutor {
    panic_name: String,
}

impl PanicOnNameExecutor {
    fn new(panic_name: &str) -> Self {
        Self {
            panic_name: panic_name.into(),
        }
    }
}

impl ActivityExecutor for PanicOnNameExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        assert!(
            activity.name != self.panic_name,
            "deliberate panic in background activity '{}'",
            activity.name
        );
        ActivityOutcome {
            success: true,
            output: None,
            error: None,
            duration_ms: 0,
        }
    }
}

// ── Experiment builder helpers ────────────────────────────────

fn with_pause_before(mut activity: Activity, secs: f64) -> Activity {
    activity.pause_before_s = Some(secs);
    activity
}

fn with_pause_after(mut activity: Activity, secs: f64) -> Activity {
    activity.pause_after_s = Some(secs);
    activity
}

// ═══════════════════════════════════════════════════════════════
// Test: background activities run concurrently with foreground
// ═══════════════════════════════════════════════════════════════

/// A background activity starts while a foreground activity is still
/// executing: the foreground one does not return before it has seen the
/// background one start.
#[test]
fn background_activities_run_concurrently() {
    let executor = OverlapExecutor::new("fg-slow", "bg-fast");
    let overlapped = Arc::clone(&executor.overlapped);
    let executor: Arc<dyn ActivityExecutor> = Arc::new(executor);
    let controls = Arc::new(ControlRegistry::new());

    let exp = minimal_experiment(vec![action("fg-slow"), background_action("bg-fast")]);

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.method_results.len(), 2);
    assert!(
        overlapped.load(Ordering::SeqCst),
        "the background activity should start before the foreground one completes"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test: pause_before_s delays activity start
// ═══════════════════════════════════════════════════════════════

/// Verify that `pause_before_s` delays the start of an activity by at least
/// the specified number of seconds.
#[test]
fn pause_before_s_delays_activity_start() {
    let pause_s = 0.05; // 50 ms

    let executor = TimestampExecutor::new();
    let timestamps = executor.timestamps();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(executor);
    let controls = Arc::new(ControlRegistry::new());

    let exp = minimal_experiment(vec![
        action("before-pause"),
        with_pause_before(action("paused-step"), pause_s),
    ]);

    let _wall_start = Instant::now();
    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.method_results.len(), 2);

    let ts = timestamps.lock().unwrap();
    let step1_start = *ts
        .get("before-pause")
        .expect("before-pause should have run");
    let step2_start = *ts.get("paused-step").expect("paused-step should have run");

    let gap = step2_start.saturating_duration_since(step1_start);
    assert!(
        gap >= Duration::from_millis(40), // 80% of 50 ms (allow some slack)
        "pause_before_s={pause_s} should delay start by at least ~40 ms; actual gap: {gap:?}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test: pause_after_s delays the next activity start
// ═══════════════════════════════════════════════════════════════

/// Verify that `pause_after_s` on an activity delays the start of the
/// subsequent activity by at least the pause duration.
#[test]
fn pause_after_s_delays_next_activity() {
    let pause_s = 0.05; // 50 ms

    let executor = TimestampExecutor::new();
    let timestamps = executor.timestamps();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(executor);
    let controls = Arc::new(ControlRegistry::new());

    let exp = minimal_experiment(vec![
        with_pause_after(action("step-with-pause-after"), pause_s),
        action("next-step"),
    ]);

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.method_results.len(), 2);

    let ts = timestamps.lock().unwrap();
    let step1_start = *ts
        .get("step-with-pause-after")
        .expect("step 1 should have run");
    let step2_start = *ts.get("next-step").expect("next-step should have run");

    let gap = step2_start.saturating_duration_since(step1_start);
    assert!(
        gap >= Duration::from_millis(40), // 80% of 50 ms
        "pause_after_s={pause_s} should delay next activity by at least ~40 ms; actual gap: {gap:?}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test: panicking background thread produces Failed result
// ═══════════════════════════════════════════════════════════════

/// Verify that a background activity that panics during execution produces a
/// `Failed` `ActivityResult` rather than propagating the panic to the caller.
#[test]
fn background_activity_panic_produces_failed_result() {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(PanicOnNameExecutor::new("bg-panic"));
    let controls = Arc::new(ControlRegistry::new());

    let exp = minimal_experiment(vec![action("fg-ok"), background_action("bg-panic")]);

    // Must not propagate the panic
    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.method_results.len(), 2);

    // The foreground activity should have succeeded
    let fg_result = journal
        .method_results
        .iter()
        .find(|r| r.name == "fg-ok")
        .expect("fg-ok result must be present");
    assert_eq!(fg_result.status, ActivityStatus::Succeeded);

    // The panicking background activity should be recorded as Failed
    let bg_result = journal
        .method_results
        .iter()
        .find(|r| r.name == "background-task" || r.status == ActivityStatus::Failed)
        .expect("a Failed result must be present for the panicking background activity");
    assert_eq!(
        bg_result.status,
        ActivityStatus::Failed,
        "panicking background activity must produce Failed status"
    );
    assert!(
        bg_result.error.is_some(),
        "Failed result must include an error message"
    );
}
