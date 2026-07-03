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
