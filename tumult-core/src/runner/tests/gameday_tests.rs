//! `GameDay` runner tests.

use super::*;
use crate::controls::ControlRegistry;

// -- GameDay runner tests

#[test]
fn gameday_runs_all_experiments() {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

    let gameday = GameDay {
        title: "Test GameDay".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: None,
        experiments: vec![
            GameDayExperiment {
                path: "exp1.toon".into(),
                compliance_maps: vec![],
            },
            GameDayExperiment {
                path: "exp2.toon".into(),
                compliance_maps: vec![],
            },
        ],
        scoring: ScoringConfig::default(),
    };

    let exp1 = minimal_experiment();
    let mut exp2 = minimal_experiment();
    exp2.title = "Second experiment".into();

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());

    let result = run_gameday(
        &gameday,
        &[exp1, exp2],
        &executor,
        &controls,
        &default_config(),
    )
    .expect("gameday should succeed");

    assert_eq!(result.experiment_journals.len(), 2);
    assert_eq!(result.title, "Test GameDay");
    assert!(result.resilience_score.overall > 0.0);
    assert_eq!(result.compliance_status, "COMPLIANT");
}

#[test]
fn gameday_score_reflects_failures() {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

    let gameday = GameDay {
        title: "Mixed GameDay".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: None,
        experiments: vec![
            GameDayExperiment {
                path: "pass.toon".into(),
                compliance_maps: vec!["ART-1".into()],
            },
            GameDayExperiment {
                path: "fail.toon".into(),
                compliance_maps: vec!["ART-2".into()],
            },
        ],
        scoring: ScoringConfig::default(),
    };

    let exp_pass = minimal_experiment();
    // Failing experiment: empty method triggers RunnerError, so use
    // a hypothesis that will fail instead
    let mut exp_fail = experiment_with_hypothesis();
    if let Some(ref mut hyp) = exp_fail.steady_state_hypothesis {
        hyp.probes[0].tolerance = Some(Tolerance::Regex {
            pattern: "^NEVER_MATCH$".into(),
        });
    }

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());

    let result = run_gameday(
        &gameday,
        &[exp_pass, exp_fail],
        &executor,
        &controls,
        &default_config(),
    )
    .expect("gameday should succeed even with deviations");

    assert_eq!(result.experiment_journals.len(), 2);
    // One completed, one aborted → pass_rate = 0.5
    assert!(result.resilience_score.pass_rate < 1.0);
    // Compliance: ART-1 met (pass), ART-2 not met (fail) → 0.5
    assert!(result.resilience_score.compliance_coverage < 1.0);
    // Overall = 0.5*0.3 + 1.0*0.25 + 1.0*0.25 + 0.5*0.2 = 0.75 → PARTIAL
    assert_eq!(result.compliance_status, "PARTIAL");
}

#[test]
fn gameday_experiment_count_mismatch_returns_error() {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

    let gameday = GameDay {
        title: "Misaligned GameDay".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: None,
        experiments: vec![
            GameDayExperiment {
                path: "exp1.toon".into(),
                compliance_maps: vec!["ART-1".into()],
            },
            GameDayExperiment {
                path: "exp2.toon".into(),
                compliance_maps: vec!["ART-2".into()],
            },
        ],
        scoring: ScoringConfig::default(),
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());

    // Only one parsed experiment for two declared entries: journals could
    // not be paired with the declared experiments, so this must error.
    let result = run_gameday(
        &gameday,
        &[minimal_experiment()],
        &executor,
        &controls,
        &default_config(),
    );

    assert!(matches!(
        result,
        Err(RunnerError::ExperimentCountMismatch {
            declared: 2,
            provided: 1
        })
    ));
}

// -- GameDay robustness tests

/// Load executor that records start/stop calls.
struct RecordingLoadExecutor {
    stopped: Arc<AtomicUsize>,
}

impl RecordingLoadExecutor {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let stopped = Arc::new(AtomicUsize::new(0));
        (
            Self {
                stopped: stopped.clone(),
            },
            stopped,
        )
    }
}

impl LoadExecutor for RecordingLoadExecutor {
    fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
        Ok(LoadHandle {
            inner: Box::new(()),
        })
    }

    fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
        self.stopped.fetch_add(1, Ordering::Relaxed);
        Ok(LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 2_000_000_000,
            duration_s: 1.0,
            vus: 1,
            throughput_rps: 10.0,
            latency_p50_ms: 1.0,
            latency_p95_ms: 2.0,
            latency_p99_ms: 3.0,
            error_rate: 0.0,
            total_requests: 10,
            thresholds_met: true,
        })
    }
}

fn gameday_with_load(paths: &[&str]) -> crate::types::GameDay {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};
    GameDay {
        title: "Robust GameDay".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: Some(LoadConfig {
            tool: LoadTool::K6,
            script: std::path::PathBuf::from("load.js"),
            vus: Some(1),
            duration_s: Some(5.0),
            thresholds: HashMap::new(),
        }),
        experiments: paths
            .iter()
            .map(|p| GameDayExperiment {
                path: (*p).into(),
                compliance_maps: vec![],
            })
            .collect(),
        scoring: ScoringConfig::default(),
    }
}

#[test]
fn gameday_stops_load_and_retains_journals_when_experiment_errors() {
    let gameday = gameday_with_load(&["ok.toon", "bad.toon"]);

    let exp_ok = minimal_experiment();
    // Empty method → run_experiment returns RunnerError::EmptyMethod.
    let exp_bad = Experiment {
        method: vec![],
        ..minimal_experiment()
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let (load_exec, stopped) = RecordingLoadExecutor::new();

    let config = RunConfig {
        load_executor: Some(Arc::new(load_exec)),
        ..RunConfig::default()
    };

    let journal = run_gameday(&gameday, &[exp_ok, exp_bad], &executor, &controls, &config)
        .expect("a failing experiment must not discard the campaign's results");

    assert_eq!(
        stopped.load(Ordering::Relaxed),
        1,
        "the shared load process must be stopped even when an experiment errors"
    );
    assert_eq!(
        journal.experiment_journals.len(),
        1,
        "journals of completed experiments must be retained in the output"
    );
    assert_eq!(
        journal.experiment_journals[0].status,
        ExperimentStatus::Completed
    );
    assert!(
        journal.load_result.is_some(),
        "load results should still be collected"
    );
}

#[test]
fn gameday_cancelled_token_skips_experiments_and_stops_load() {
    let gameday = gameday_with_load(&["a.toon", "b.toon"]);

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    let (load_exec, stopped) = RecordingLoadExecutor::new();

    let token = CancellationToken::new();
    token.cancel();

    let config = RunConfig {
        cancellation_token: Some(token),
        load_executor: Some(Arc::new(load_exec)),
        ..RunConfig::default()
    };

    let journal = run_gameday(
        &gameday,
        &[minimal_experiment(), minimal_experiment()],
        &executor,
        &controls,
        &config,
    )
    .expect("a cancelled campaign still returns its journal");

    assert!(
        journal.experiment_journals.is_empty(),
        "no experiment should start once the token is cancelled"
    );
    assert_eq!(
        stopped.load(Ordering::Relaxed),
        1,
        "the shared load process must be stopped on the cancellation path"
    );
}
