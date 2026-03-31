//! Integration tests for background activity execution.
//!
//! Verifies:
//! - Background activities run concurrently with foreground activities
//! - `pause_before_s` delays activity start by at least the specified duration
//! - `pause_after_s` delays the start of the next activity by at least the specified duration
//! - A panicking background thread produces a `Failed` `ActivityResult`

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;
use tumult_test_utils::{action, background_action, minimal_experiment};

// ── Helper executor types ─────────────────────────────────────

/// Records the wall-clock instant at which each activity starts executing.
struct TimestampExecutor {
    timestamps: Arc<Mutex<HashMap<String, Instant>>>,
    durations: HashMap<String, Duration>,
}

impl TimestampExecutor {
    fn new() -> Self {
        Self {
            timestamps: Arc::new(Mutex::new(HashMap::new())),
            durations: HashMap::new(),
        }
    }

    fn with_duration(mut self, name: &str, duration: Duration) -> Self {
        self.durations.insert(name.into(), duration);
        self
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

        if let Some(dur) = self.durations.get(&activity.name) {
            std::thread::sleep(*dur);
        }

        ActivityOutcome {
            success: true,
            output: Some("ok".into()),
            error: None,
            duration_ms: 0,
        }
    }
}

/// Executor that records a counter increment per execution call.
#[allow(dead_code)] // Reserved for future concurrency count tests
struct CountingExecutor {
    count: Arc<AtomicUsize>,
}

#[allow(dead_code)]
impl CountingExecutor {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ActivityExecutor for CountingExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        self.count.fetch_add(1, Ordering::Relaxed);
        ActivityOutcome {
            success: true,
            output: None,
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

/// Verify that a background activity starts before a slow foreground activity
/// completes — demonstrating true concurrency.
///
/// Layout:
///   - foreground: "fg-slow" sleeps 80 ms
///   - background: "bg-fast" sleeps 0 ms
///
/// If execution is concurrent, "bg-fast" should record a start timestamp
/// that is earlier than `fg-slow`'s start + 80 ms (i.e. bg doesn't wait
/// for fg to finish before starting).
#[test]
fn background_activities_run_concurrently() {
    let executor = TimestampExecutor::new().with_duration("fg-slow", Duration::from_millis(80));
    let timestamps = executor.timestamps();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(executor);
    let controls = Arc::new(ControlRegistry::new());

    let exp = minimal_experiment(vec![action("fg-slow"), background_action("bg-fast")]);

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.method_results.len(), 2);

    let ts = timestamps.lock().unwrap();
    let fg_start = *ts.get("fg-slow").expect("fg-slow should have run");
    let bg_start = *ts.get("bg-fast").expect("bg-fast should have run");

    // If background ran concurrently, it should start before fg-slow completes
    // (i.e. within the 80 ms window of fg-slow executing).
    let bg_relative_to_fg = bg_start.saturating_duration_since(fg_start);
    assert!(
        bg_relative_to_fg < Duration::from_millis(80),
        "background activity should start before foreground completes; \
         bg started {bg_relative_to_fg:?} after fg"
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
