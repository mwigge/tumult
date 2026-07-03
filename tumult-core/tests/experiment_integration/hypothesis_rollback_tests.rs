//! Baselined-hypothesis tolerance and abort/rollback tests (Tasks 75 & 76).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::common::*;

// ═══════════════════════════════════════════════════════════════
// Task 75: Baselined hypothesis — derive then compare
// ═══════════════════════════════════════════════════════════════

#[test]
fn baselined_hypothesis_with_range_tolerance() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Latency within range",
        vec![Activity {
            name: "latency-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/metrics".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            // Simulating derived tolerance: latency between 20-80ms
            tolerance: Some(Tolerance::Range {
                from: 20.0,
                to: 80.0,
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
    ));

    // Probe returns 45.0 (within range)
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(
        MockPlugin::new()
            .on("latency-probe", true, Some("45.0"))
            .default_output("200"),
    );
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    assert!(journal.steady_state_after.as_ref().unwrap().met);
}

#[test]
fn baselined_hypothesis_fails_when_outside_range() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Latency within range",
        vec![Activity {
            name: "latency-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/metrics".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Range {
                from: 20.0,
                to: 80.0,
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
    ));

    // Probe returns 150.0 (outside range) — hypothesis fails before method
    let plugin: Arc<dyn ActivityExecutor> =
        Arc::new(MockPlugin::new().on("latency-probe", true, Some("150.0")));
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(!journal.steady_state_before.as_ref().unwrap().met);
    assert!(journal.method_results.is_empty());
}

// ═══════════════════════════════════════════════════════════════
// Task 76: Hypothesis failure → abort → rollbacks
// ═══════════════════════════════════════════════════════════════

#[test]
fn hypothesis_failure_aborts_and_runs_rollbacks() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "System is healthy",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));
    exp.rollbacks = vec![action("emergency-cleanup"), action("notify-ops")];

    // Health check returns 503 → hypothesis fails → abort
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(
        MockPlugin::new()
            .on("health-check", true, Some("503"))
            .on("emergency-cleanup", true, Some("ok"))
            .on("notify-ops", true, Some("ok")),
    );

    let mut controls = ControlRegistry::new();
    let (logger, events) = EventLog::new();
    controls.register(Box::new(logger));
    let controls = Arc::new(controls);

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Status should be aborted
    assert_eq!(journal.status, ExperimentStatus::Aborted);

    // Hypothesis before should have failed
    assert!(!journal.steady_state_before.as_ref().unwrap().met);

    // Method should NOT have executed
    assert!(journal.method_results.is_empty());

    // Rollbacks SHOULD have executed (abort is treated as deviation)
    assert_eq!(journal.rollback_results.len(), 2);
    assert_eq!(journal.rollback_results[0].name, "emergency-cleanup");
    assert_eq!(journal.rollback_results[1].name, "notify-ops");

    // No hypothesis after (never reached)
    assert!(journal.steady_state_after.is_none());

    // Verify lifecycle events
    let events = events.lock().unwrap();
    assert!(events.contains(&LifecycleEvent::BeforeRollback));
    assert!(events.contains(&LifecycleEvent::AfterRollback));
    // Method events should NOT be present
    assert!(!events.contains(&LifecycleEvent::BeforeMethod));
}

// ── Executors used in phase-aware tests ───────────────────────

struct PhaseAwareExecutor {
    call_count: AtomicUsize,
}
impl ActivityExecutor for PhaseAwareExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        let count = self.call_count.fetch_add(1, Ordering::Relaxed);
        match count {
            0 => ActivityOutcome {
                // Hypothesis before: pass
                success: true,
                output: Some("200".into()),
                error: None,
                duration_ms: 10,
            },
            1 => ActivityOutcome {
                // Method: succeed
                success: true,
                output: Some("fault injected".into()),
                error: None,
                duration_ms: 100,
            },
            2 => ActivityOutcome {
                // Hypothesis after: FAIL (system degraded)
                success: true,
                output: Some("503".into()),
                error: None,
                duration_ms: 10,
            },
            _ => ActivityOutcome {
                // Rollback: succeed
                success: true,
                output: Some("rolled back".into()),
                error: None,
                duration_ms: 10,
            },
        }
    }
}

#[test]
fn hypothesis_after_failure_causes_deviation_with_rollback() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "System is healthy",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));
    exp.rollbacks = vec![action("rollback-action")];

    let executor: Arc<dyn ActivityExecutor> = Arc::new(PhaseAwareExecutor {
        call_count: AtomicUsize::new(0),
    });
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    assert!(!journal.steady_state_after.as_ref().unwrap().met);
    assert_eq!(journal.method_results.len(), 1);
    // OnDeviation strategy: rollbacks should execute
    assert_eq!(journal.rollback_results.len(), 1);
}
