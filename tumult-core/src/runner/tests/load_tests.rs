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
            execution: crate::types::LoadExecution::default(),
        })
    }
}

pub(super) fn experiment_with_load() -> Experiment {
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

pub(super) fn load_result(total_requests: u64, thresholds_met: bool) -> LoadResult {
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
        execution: crate::types::LoadExecution::default(),
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

    pub(super) fn answering(
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

/// A load result whose plan fell short: 35 of 60 windows.
fn truncated_load() -> LoadResult {
    LoadResult {
        execution: crate::types::LoadExecution {
            planned_windows: Some(60),
            completed_windows: Some(35),
            reasons: vec!["the method ended before 25 of 60 windows started".into()],
            ..crate::types::LoadExecution::default()
        },
        ..load_result(3500, false)
    }
}

#[test]
fn a_truncated_plan_is_a_deviation_and_is_journaled() {
    let (journal, _) = run_with_load(Ok(truncated_load()));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let execution = &journal.load_result.as_ref().expect("load result").execution;
    assert_eq!(execution.planned_windows, Some(60));
    assert_eq!(execution.completed_windows, Some(35));
    assert!(execution.truncated());
}

/// Runs a probe-only method (no fault is ever started) against a load
/// with the scripted outcome, and reports whether the rollbacks ran.
fn probe_only_run(outcome: Result<LoadResult, String>) -> (Journal, bool) {
    struct Recording(Arc<std::sync::Mutex<Vec<String>>>);
    impl ActivityExecutor for Recording {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            self.0.lock().expect("lock").push(activity.name.clone());
            ActivityOutcome {
                success: true,
                output: Some("200".into()),
                error: None,
                duration_ms: 0,
            }
        }
    }
    let mut exp = experiment_with_load();
    exp.method = vec![test_probe("watch")];
    exp.rollbacks = vec![test_action("cleanup")];
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Recording(calls.clone()));
    let (load, _) = ScriptedLoad::answering(outcome);
    let config = RunConfig {
        load_executor: Some(Arc::new(load)),
        ..RunConfig::default()
    };
    let journal =
        run_experiment(&exp, &executor, &Arc::new(ControlRegistry::new()), &config).unwrap();
    let rolled_back = calls.lock().expect("lock").iter().any(|n| n == "cleanup");
    (journal, rolled_back)
}

#[test]
fn a_load_without_evidence_fails_the_run_and_still_rolls_back() {
    // `Failed` outranks `Deviated`, but a probe-only method must not lose
    // the rollbacks the deviation would have run.
    let (journal, rolled_back) = probe_only_run(Ok(load_result(0, false)));
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(rolled_back, "the failed status hid the rollbacks");

    let (journal, rolled_back) = probe_only_run(Err("no summary".into()));
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(rolled_back);
}

#[test]
fn a_missed_threshold_rolls_back_and_a_clean_run_does_not() {
    let (journal, rolled_back) = probe_only_run(Ok(load_result(100, false)));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(rolled_back);

    let (journal, rolled_back) = probe_only_run(Ok(load_result(100, true)));
    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(!rolled_back);
}

/// A load result as an aborted flat load leaves it: marked, with the
/// partial evidence of the window that was cut.
pub(super) fn aborted_load() -> LoadResult {
    LoadResult {
        execution: crate::types::LoadExecution {
            planned_windows: Some(1),
            completed_windows: Some(0),
            aborted: true,
            reasons: vec!["load aborted after 0 of 1 windows completed".into()],
            ..crate::types::LoadExecution::default()
        },
        ..load_result(64, false)
    }
}

#[test]
fn the_load_span_says_when_a_load_was_aborted_or_truncated() {
    let value = |attributes: &[opentelemetry::KeyValue], key: &str| {
        attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    };
    let aborted = load::result_attributes(&aborted_load());
    assert_eq!(
        value(&aborted, "resilience.load.aborted").as_deref(),
        Some("true")
    );
    assert_eq!(
        value(&aborted, "resilience.load.planned_windows").as_deref(),
        Some("1")
    );
    assert_eq!(
        value(&aborted, "resilience.load.completed_windows").as_deref(),
        Some("0")
    );
    assert_eq!(
        value(&aborted, "resilience.load.reason").as_deref(),
        Some("load aborted after 0 of 1 windows completed")
    );

    let truncated = load::result_attributes(&truncated_load());
    assert_eq!(
        value(&truncated, "resilience.load.truncated").as_deref(),
        Some("true")
    );
    assert_eq!(
        value(&truncated, "resilience.load.aborted").as_deref(),
        Some("false")
    );

    let clean = load::result_attributes(&load_result(100, true));
    assert_eq!(
        value(&clean, "resilience.load.truncated").as_deref(),
        Some("false")
    );
    assert!(value(&clean, "resilience.load.reason").is_none());
    assert!(value(&clean, "resilience.load.planned_windows").is_none());

    // The path with no result at all is on the span too.
    let failed = load::failure_attributes(load::LoadEnd::Abort, "oha worker thread panicked");
    assert_eq!(
        value(&failed, "resilience.load.error").as_deref(),
        Some("oha worker thread panicked")
    );
    assert_eq!(
        value(&failed, "resilience.load.aborted").as_deref(),
        Some("true")
    );
    assert_eq!(
        value(&failed, "resilience.load.thresholds_met").as_deref(),
        Some("false")
    );
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
    assert!(journal.load_result.is_some(), "the load's evidence is kept");
}

/// A load tool that cannot be started, as when its binary is missing.
pub(super) struct NoStart;

/// What [`NoStart`] says when it is asked to start.
pub(super) const NO_START: &str = "failed to start oha (oha): No such file or directory";

impl LoadExecutor for NoStart {
    fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
        Err(NO_START.into())
    }

    fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        Err("a load that never started has nothing to stop".into())
    }
}

/// A declared load runs with the adapter of the tool it names when the run
/// is given no load executor.
#[cfg(unix)]
#[test]
fn a_declared_load_runs_with_the_adapter_of_its_tool() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let tools = tempfile::tempdir().expect("tools directory");
    // A stand-in for oha: one window of 40 answered requests.
    let summary = r#"{"summary":{"successRate":1.0,"total":1.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":40.0,"totalData":1,"sizePerRequest":1,"sizePerSec":1},"responseTimeHistogram":{},"latencyPercentiles":{"p50":0.02,"p95":0.08,"p99":0.1},"rps":{},"details":{},"statusCodeDistribution":{"200":40},"errorDistribution":{}}"#;
    let oha = tumult_test_utils::fake_bin(
        tools.path(),
        "oha",
        &format!("[ \"$1\" = \"--version\" ] && exit 0\nprintf '%s' '{summary}'"),
    );
    env.set("TUMULT_OHA_BINARY", &oha);
    let mut exp = minimal_experiment();
    exp.load = Some(LoadConfig {
        tool: LoadTool::Oha,
        script: std::path::PathBuf::from("http://127.0.0.1:9/health"),
        vus: Some(1),
        duration_s: Some(1.0),
        thresholds: HashMap::new(),
        profile: None,
    });
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());

    let run = run_experiment(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
    );

    let journal = run.expect("the run has a load executor");
    let load = journal.load_result.expect("the load left its result");
    assert_eq!(load.tool, LoadTool::Oha);
    assert_eq!(load.total_requests, 40);
}
