//! Load-testing phase tests.

use super::*;
use crate::controls::ControlRegistry;

// -- Load testing phase tests

struct MockLoadExecutor {
    started: Arc<std::sync::Mutex<bool>>,
    stopped: Arc<std::sync::Mutex<bool>>,
}

impl MockLoadExecutor {
    fn new() -> (
        Self,
        Arc<std::sync::Mutex<bool>>,
        Arc<std::sync::Mutex<bool>>,
    ) {
        let started = Arc::new(std::sync::Mutex::new(false));
        let stopped = Arc::new(std::sync::Mutex::new(false));
        (
            Self {
                started: started.clone(),
                stopped: stopped.clone(),
            },
            started,
            stopped,
        )
    }
}

impl LoadExecutor for MockLoadExecutor {
    fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
        *self.started.lock().expect("lock") = true;
        Ok(LoadHandle {
            inner: Box::new(()),
        })
    }

    fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        *self.stopped.lock().expect("lock") = true;
        Ok(LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 2_000_000_000,
            duration_s: 1.0,
            vus: 5,
            throughput_rps: 100.0,
            latency_p50_ms: 10.0,
            latency_p95_ms: 50.0,
            latency_p99_ms: 100.0,
            error_rate: 0.01,
            total_requests: 100,
            thresholds_met: true,
            samples: vec![],
        })
    }
}

fn experiment_with_load() -> Experiment {
    let mut exp = experiment_with_hypothesis();
    exp.load = Some(LoadConfig {
        tool: LoadTool::K6,
        script: std::path::PathBuf::from("test.js"),
        vus: Some(5),
        duration_s: Some(10.0),
        thresholds: HashMap::new(),
        profile: None,
    });
    exp
}

#[test]
fn load_result_none_when_no_load_config() {
    let exp = minimal_experiment();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

    assert!(journal.load_result.is_none());
}

#[test]
fn load_result_populated_when_load_executor_present() {
    let exp = experiment_with_load();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let (mock_load, started, stopped) = MockLoadExecutor::new();

    let config = RunConfig {
        load_executor: Some(Arc::new(mock_load)),
        ..RunConfig::default()
    };

    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

    // Load executor was called
    assert!(
        *started.lock().expect("lock"),
        "load should have been started"
    );
    assert!(
        *stopped.lock().expect("lock"),
        "load should have been stopped"
    );

    // Load result populated in journal
    assert!(
        journal.load_result.is_some(),
        "journal should have load_result"
    );
    let lr = journal.load_result.as_ref().expect("load_result");
    assert_eq!(lr.vus, 5);
    assert_eq!(lr.total_requests, 100);
    assert!(lr.thresholds_met);
}

#[test]
fn load_not_started_when_hypothesis_fails() {
    let mut exp = experiment_with_load();
    // Make hypothesis tolerance impossible
    if let Some(ref mut hyp) = exp.steady_state_hypothesis {
        hyp.probes[0].tolerance = Some(Tolerance::Regex {
            pattern: "^IMPOSSIBLE$".into(),
        });
    }

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let (mock_load, started, _stopped) = MockLoadExecutor::new();

    let config = RunConfig {
        load_executor: Some(Arc::new(mock_load)),
        ..RunConfig::default()
    };

    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(
        !*started.lock().expect("lock"),
        "load should NOT start when hypothesis fails"
    );
    assert!(journal.load_result.is_none());
}

// -- How the load is ended, and what its outcome does to the run

fn load_result(total_requests: u64, thresholds_met: bool) -> LoadResult {
    LoadResult {
        tool: LoadTool::Oha,
        started_at_ns: 1_000_000_000,
        ended_at_ns: 2_000_000_000,
        duration_s: 1.0,
        vus: 5,
        throughput_rps: 100.0,
        latency_p50_ms: 10.0,
        latency_p95_ms: 50.0,
        latency_p99_ms: 100.0,
        error_rate: 0.0,
        total_requests,
        thresholds_met,
        samples: vec![],
    }
}

/// Load executor that records how it was ended in a shared event log and
/// answers with a scripted outcome.
pub(super) struct ScriptedLoad {
    pub(super) events: Arc<std::sync::Mutex<Vec<String>>>,
    pub(super) outcome: Result<LoadResult, String>,
}

impl ScriptedLoad {
    pub(super) fn healthy() -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
        Self::answering(Ok(load_result(100, true)))
    }

    fn answering(
        outcome: Result<LoadResult, String>,
    ) -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
                outcome,
            },
            events,
        )
    }

    fn record(&self, event: &str) -> Result<LoadResult, String> {
        self.events.lock().expect("lock").push(event.into());
        self.outcome.clone()
    }
}

impl LoadExecutor for ScriptedLoad {
    fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
        Ok(LoadHandle {
            inner: Box::new(()),
        })
    }

    fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        self.record("load.stop")
    }

    fn abort(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        self.record("load.abort")
    }
}

fn run_with_load(outcome: Result<LoadResult, String>) -> (Journal, Vec<String>) {
    let exp = experiment_with_load();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let (load, events) = ScriptedLoad::answering(outcome);
    let config = RunConfig {
        load_executor: Some(Arc::new(load)),
        ..RunConfig::default()
    };
    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();
    let events = events.lock().expect("lock").clone();
    (journal, events)
}

#[test]
fn a_method_that_finishes_stops_the_load_normally() {
    let (journal, events) = run_with_load(Ok(load_result(100, true)));
    assert_eq!(events, vec!["load.stop"]);
    assert_eq!(journal.status, ExperimentStatus::Completed);
}

#[test]
fn a_load_whose_results_could_not_be_collected_fails_the_run() {
    let (journal, _) = run_with_load(Err("oha run was cut before any window completed".into()));
    assert!(journal.load_result.is_none());
    assert_eq!(journal.status, ExperimentStatus::Failed);
}

#[test]
fn a_load_that_sent_no_request_fails_the_run() {
    let (journal, _) = run_with_load(Ok(load_result(0, false)));
    assert!(
        journal.load_result.is_some(),
        "the empty result is journaled"
    );
    assert_eq!(journal.status, ExperimentStatus::Failed);
}

#[test]
fn a_load_that_missed_its_thresholds_is_a_deviation() {
    let (journal, _) = run_with_load(Ok(load_result(100, false)));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
}

#[test]
fn an_interrupted_method_aborts_the_load_before_the_rollbacks() {
    /// Cancels the run from inside the first method action and logs the
    /// rollback into the same event log as the load executor.
    struct CancelInMethod {
        token: tokio_util::sync::CancellationToken,
        events: Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl ActivityExecutor for CancelInMethod {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            if activity.name == "cleanup" {
                self.events.lock().expect("lock").push("rollback".into());
            }
            if activity.name == "inject" {
                self.token.cancel();
            }
            ActivityOutcome {
                success: true,
                output: Some("200".into()),
                error: None,
                duration_ms: 0,
            }
        }
    }

    let mut exp = experiment_with_load();
    exp.method = vec![test_action("inject"), test_action("never")];
    exp.rollbacks = vec![test_action("cleanup")];
    let token = tokio_util::sync::CancellationToken::new();
    let (load, events) = ScriptedLoad::healthy();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(CancelInMethod {
        token: token.clone(),
        events: events.clone(),
    });
    let config = RunConfig {
        cancellation_token: Some(token),
        load_executor: Some(Arc::new(load)),
        ..RunConfig::default()
    };

    let journal =
        run_experiment(&exp, &executor, &Arc::new(ControlRegistry::new()), &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(
        *events.lock().expect("lock"),
        vec!["load.abort", "rollback"],
        "an interrupt must end the load at once, never wait out the plan"
    );
}
