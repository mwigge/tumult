//! Auto-halt guardrail behaviour: a guard that breaches its safe-condition
//! tolerance during the fault window cancels the method, runs rollbacks, and
//! marks the run `Halted`.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

// The guard worker drops its thread-local sender only after returning from
// guard evaluation. This proves that cancellation has been applied before the
// method resumes, without assuming when the OS schedules the monitor thread.
thread_local! {
    static GUARD_EXIT: std::cell::RefCell<Option<mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

/// Executor where the guard probe always reports an unsafe value (breaching a
/// `regex "ok"` safe-condition), method actions wait for the guard, and rollbacks are
/// recorded. Distinguishes roles by activity type / name.
struct HaltingExecutor {
    method_calls: Arc<Mutex<Vec<String>>>,
    rollback_ran: Arc<AtomicBool>,
    guard_exit_sender: Mutex<Option<mpsc::Sender<()>>>,
    guard_exit_receiver: Mutex<mpsc::Receiver<()>>,
}

impl ActivityExecutor for HaltingExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match activity.activity_type {
            // The guard's probe: always outside the safe condition.
            ActivityType::Probe => {
                let sender = self
                    .guard_exit_sender
                    .lock()
                    .unwrap()
                    .take()
                    .expect("unsafe guard must halt after its first probe");
                GUARD_EXIT.with(|slot| *slot.borrow_mut() = Some(sender));
                ActivityOutcome {
                    success: true,
                    output: Some("UNSAFE".into()),
                    error: None,
                    duration_ms: 1,
                }
            }
            ActivityType::Action => {
                if activity.name.starts_with("rollback") {
                    self.rollback_ran.store(true, Ordering::SeqCst);
                } else {
                    self.method_calls
                        .lock()
                        .unwrap()
                        .push(activity.name.clone());
                    assert_eq!(
                        self.guard_exit_receiver
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5)),
                        Err(mpsc::RecvTimeoutError::Disconnected),
                        "guard worker must finish before the method continues"
                    );
                }
                ActivityOutcome {
                    success: true,
                    output: Some("ok".into()),
                    error: None,
                    duration_ms: 0,
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

/// A load executor that, when aborted, notes whether the rollback had
/// already run and answers with a scripted outcome.
struct AbortedLoad {
    rollback_ran: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<String>>>,
    outcome: Result<LoadResult, String>,
}

impl LoadExecutor for AbortedLoad {
    fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
        Ok(LoadHandle {
            inner: Box::new(()),
        })
    }

    fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        self.events.lock().unwrap().push("load.stop".into());
        self.outcome.clone()
    }

    fn abort(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        let order = if self.rollback_ran.load(Ordering::SeqCst) {
            "load.abort after rollback"
        } else {
            "load.abort before rollback"
        };
        self.events.lock().unwrap().push(order.into());
        self.outcome.clone()
    }
}

/// Halt an experiment with a flat load whose abort answers `outcome`.
fn halt_with_load(outcome: Result<LoadResult, String>) -> (Journal, Vec<String>, bool) {
    let mut exp = minimal_experiment();
    exp.guards = vec![health_guard()];
    exp.method = vec![test_action("action-1"), test_action("action-2")];
    exp.rollbacks = vec![test_action("rollback-1")];
    exp.load = Some(LoadConfig {
        tool: LoadTool::Oha,
        script: std::path::PathBuf::from("http://127.0.0.1:1/"),
        vus: Some(5),
        duration_s: Some(600.0),
        thresholds: HashMap::new(),
        profile: None,
    });

    let rollback_ran = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (guard_exit_sender, guard_exit_receiver) = mpsc::channel();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HaltingExecutor {
        method_calls: Arc::new(Mutex::new(Vec::new())),
        rollback_ran: rollback_ran.clone(),
        guard_exit_sender: Mutex::new(Some(guard_exit_sender)),
        guard_exit_receiver: Mutex::new(guard_exit_receiver),
    });
    let config = RunConfig {
        load_executor: Some(Arc::new(AbortedLoad {
            rollback_ran: rollback_ran.clone(),
            events: events.clone(),
            outcome,
        })),
        ..default_config()
    };
    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &config,
        &fast_sampling(),
    )
    .unwrap();
    let events = events.lock().unwrap().clone();
    (journal, events, rollback_ran.load(Ordering::SeqCst))
}

#[test]
fn a_halted_flat_load_is_aborted_before_the_rollback_and_journaled() {
    let (journal, events, rolled_back) = halt_with_load(Ok(super::load_tests::aborted_load()));

    assert_eq!(journal.status, ExperimentStatus::Halted);
    assert_eq!(events, vec!["load.abort before rollback"]);
    assert!(rolled_back);
    let load = journal.load_result.expect("the abort is journaled");
    assert!(load.execution.aborted, "the journal must mark the abort");
    assert_eq!(load.total_requests, 64, "partial evidence is kept");
    assert!(!load.thresholds_met);
}

#[test]
fn a_halt_rolls_back_even_when_the_aborted_load_left_nothing() {
    let (journal, events, rolled_back) = halt_with_load(Err("oha worker thread panicked".into()));

    assert_eq!(journal.status, ExperimentStatus::Halted);
    assert_eq!(events, vec!["load.abort before rollback"]);
    assert!(rolled_back, "a load error must not cost the rollback");
    assert!(journal.load_result.is_none());
}

#[test]
fn a_halted_run_logs_its_status_in_the_completed_event() {
    let mut journal = None;
    let events = super::log_capture::logged(|| {
        journal = Some(halt_with_load(Err("aborted".into())).0);
    });
    let journal = journal.expect("the run returned a journal");

    assert_eq!(journal.status, ExperimentStatus::Halted);
    let completed: Vec<_> = events
        .iter()
        .filter(|event| event.event == "experiment.completed")
        .collect();
    assert_eq!(completed.len(), 1, "{events:?}");
    assert_eq!(completed[0].field("status"), Some("halted"));
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
    let (guard_exit_sender, guard_exit_receiver) = mpsc::channel();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HaltingExecutor {
        method_calls: method_calls.clone(),
        rollback_ran: rollback_ran.clone(),
        guard_exit_sender: Mutex::new(Some(guard_exit_sender)),
        guard_exit_receiver: Mutex::new(guard_exit_receiver),
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

#[test]
fn guard_breach_aborts_the_load_instead_of_waiting_for_its_plan() {
    let mut exp = minimal_experiment();
    exp.guards = vec![health_guard()];
    exp.method = vec![test_action("action-1"), test_action("action-2")];
    exp.rollbacks = vec![test_action("rollback-1")];
    exp.load = Some(LoadConfig {
        tool: LoadTool::Oha,
        script: "http://127.0.0.1:1/".into(),
        vus: Some(4),
        // A flat ten-minute load a normal stop would sit out.
        duration_s: Some(600.0),
        thresholds: HashMap::new(),
        profile: None,
    });

    let (guard_exit_sender, guard_exit_receiver) = mpsc::channel();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HaltingExecutor {
        method_calls: Arc::new(Mutex::new(Vec::new())),
        rollback_ran: Arc::new(AtomicBool::new(false)),
        guard_exit_sender: Mutex::new(Some(guard_exit_sender)),
        guard_exit_receiver: Mutex::new(guard_exit_receiver),
    });
    let (load, events) = super::load_tests::ScriptedLoad::healthy();
    let config = RunConfig {
        load_executor: Some(Arc::new(load)),
        ..default_config()
    };

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &config,
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Halted);
    assert_eq!(*events.lock().unwrap(), vec!["load.abort"]);
    assert!(journal.load_result.is_some(), "what was measured is kept");
}

/// A halted run is closed like any other: its version is observed once,
/// over the window that ended with the halt.
#[test]
fn a_halted_run_observes_its_version_when_it_ends() {
    use super::version_tests::{checkout, config_with, ScriptedResolver};

    let mut exp = minimal_experiment();
    exp.guards = vec![health_guard()];
    exp.method = vec![test_action("action-1"), test_action("action-2")];
    exp.target = Some(checkout(None));
    let resolver = ScriptedResolver::answering(Some("1.5.1"));
    let (guard_exit_sender, guard_exit_receiver) = mpsc::channel();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HaltingExecutor {
        method_calls: Arc::new(Mutex::new(Vec::new())),
        rollback_ran: Arc::new(AtomicBool::new(false)),
        guard_exit_sender: Mutex::new(Some(guard_exit_sender)),
        guard_exit_receiver: Mutex::new(guard_exit_receiver),
    });
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &config_with(&resolver),
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Halted);
    let questions = resolver.questions();
    assert_eq!(
        questions[0].ended_at_ns, journal.ended_at_ns,
        "the end of the run"
    );
    assert_eq!(questions.len(), 1, "{questions:?}");
    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.1".into(),
            version_source: VersionSource::Observed,
            shares: Vec::new(),
        })
    );
}

#[test]
fn a_guard_whose_probe_panics_halts_the_run() {
    // A guard that cannot run its probe cannot see the target. The run
    // stops as on a breach, instead of going on without its guard.
    struct PanickingProbe;
    impl ActivityExecutor for PanickingProbe {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            match activity.activity_type {
                ActivityType::Probe => panic!("the guard probe broke"),
                ActivityType::Action => ActivityOutcome {
                    success: true,
                    output: Some("done".into()),
                    error: None,
                    duration_ms: 1,
                },
            }
        }
    }

    // A guard with no tolerance is safe when its probe succeeds, so the
    // probe that panicked must not read as one that succeeded.
    let mut no_tolerance = health_guard();
    no_tolerance.probe.tolerance = None;
    for guard in [health_guard(), no_tolerance] {
        let mut exp = minimal_experiment();
        exp.guards = vec![guard];
        exp.method = vec![test_action("action-1")];

        let executor: Arc<dyn ActivityExecutor> = Arc::new(PanickingProbe);
        let journal = run_experiment_with_sampling(
            &exp,
            &executor,
            &Arc::new(ControlRegistry::new()),
            &default_config(),
            &fast_sampling(),
        )
        .unwrap();

        assert_eq!(journal.status, ExperimentStatus::Halted);
        assert_eq!(journal.halt.expect("a halt record").guard_name, "safety");
    }
}
