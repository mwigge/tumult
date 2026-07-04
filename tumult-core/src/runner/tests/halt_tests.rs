//! Auto-halt guardrail behaviour: a guard that breaches its safe-condition
//! tolerance during the fault window cancels the method, runs rollbacks, and
//! marks the run `Halted`.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Duration;

/// Executor where the guard probe always reports an unsafe value (breaching a
/// `regex "ok"` safe-condition), method actions are slow, and rollbacks are
/// recorded. Distinguishes roles by activity type / name.
struct HaltingExecutor {
    method_calls: Arc<Mutex<Vec<String>>>,
    rollback_ran: Arc<AtomicBool>,
}

impl ActivityExecutor for HaltingExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match activity.activity_type {
            // The guard's probe: always outside the safe condition.
            ActivityType::Probe => ActivityOutcome {
                success: true,
                output: Some("UNSAFE".into()),
                error: None,
                duration_ms: 1,
            },
            ActivityType::Action => {
                if activity.name.starts_with("rollback") {
                    self.rollback_ran.store(true, Ordering::SeqCst);
                } else {
                    self.method_calls
                        .lock()
                        .unwrap()
                        .push(activity.name.clone());
                    // Slow enough that the 10ms-interval guard monitor breaches
                    // and cancels the method before every action has run.
                    std::thread::sleep(Duration::from_millis(60));
                }
                ActivityOutcome {
                    success: true,
                    output: Some("ok".into()),
                    error: None,
                    duration_ms: 60,
                }
            }
        }
    }
}

fn health_guard() -> Guard {
    let mut probe = test_probe("guard-probe");
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "ok".into(),
    });
    Guard {
        name: "safety".into(),
        probe,
        min_breaches: 1,
    }
}

#[test]
fn guard_breach_halts_experiment_and_runs_rollback() {
    let mut exp = minimal_experiment();
    exp.guards = vec![health_guard()];
    // Several method actions so early cancellation is observable.
    exp.method = vec![
        test_action("action-1"),
        test_action("action-2"),
        test_action("action-3"),
    ];
    exp.rollbacks = vec![test_action("rollback-1")];

    let method_calls = Arc::new(Mutex::new(Vec::new()));
    let rollback_ran = Arc::new(AtomicBool::new(false));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HaltingExecutor {
        method_calls: method_calls.clone(),
        rollback_ran: rollback_ran.clone(),
    });
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &default_config(),
        &fast_sampling(),
    )
    .unwrap();

    // The guard pulled the plug.
    assert_eq!(journal.status, ExperimentStatus::Halted);
    let halt = journal.halt.expect("halt record present when Halted");
    assert_eq!(halt.guard_name, "safety");

    // Rollback ran on the halt path.
    assert!(
        rollback_ran.load(Ordering::SeqCst),
        "rollback should run when a guard halts the experiment"
    );
    assert_eq!(journal.rollback_results.len(), 1);
    assert_eq!(journal.rollback_results[0].name, "rollback-1");

    // The method was cancelled early — not every action ran.
    let ran = method_calls.lock().unwrap().len();
    assert!(
        ran < 3,
        "expected early cancellation, but all {ran} method actions ran"
    );
}

#[test]
fn no_guard_breach_completes_normally() {
    // A guard whose probe stays within tolerance must not halt: the run
    // proceeds exactly as it would with no guards at all.
    struct SafeExecutor;
    impl ActivityExecutor for SafeExecutor {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            let output = match activity.activity_type {
                ActivityType::Probe => "ok", // within the safe condition
                ActivityType::Action => "done",
            };
            ActivityOutcome {
                success: true,
                output: Some(output.into()),
                error: None,
                duration_ms: 5,
            }
        }
    }

    let mut exp = minimal_experiment();
    exp.guards = vec![health_guard()];
    exp.method = vec![test_action("action-1")];

    let executor: Arc<dyn ActivityExecutor> = Arc::new(SafeExecutor);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &default_config(),
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.halt.is_none());
}
