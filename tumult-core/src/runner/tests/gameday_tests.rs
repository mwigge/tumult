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

// -- GameDay scoring tests

/// Probes answer; every action of the method fails.
struct FailingMethodExecutor;

impl ActivityExecutor for FailingMethodExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let is_probe = activity.activity_type == ActivityType::Probe;
        ActivityOutcome {
            success: is_probe,
            output: Some("200".into()),
            error: None,
            duration_ms: 1,
        }
    }
}

#[test]
fn a_run_whose_method_failed_is_not_a_recovery_within_target() {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

    let gameday = GameDay {
        title: "Failed method".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: None,
        experiments: vec![GameDayExperiment {
            path: "fail.toon".into(),
            compliance_maps: vec![],
        }],
        scoring: ScoringConfig::default(),
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(FailingMethodExecutor);

    let result = run_gameday(
        &gameday,
        &[experiment_with_hypothesis()],
        &executor,
        &Arc::new(ControlRegistry::new()),
        &default_config(),
    )
    .expect("gameday should run");

    let journal = &result.experiment_journals[0];
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(journal.post_result.is_some(), "the probes were sampled");
    assert!(
        result.resilience_score.recovery_compliance.abs() < f64::EPSILON,
        "got {}",
        result.resilience_score.recovery_compliance
    );
}

// -- The shared load of a game day

/// A game day of one experiment under `load`.
fn gameday_under(load: Option<LoadConfig>) -> crate::types::GameDay {
    use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

    GameDay {
        title: "Under load".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load,
        experiments: vec![GameDayExperiment {
            path: "exp.toon".into(),
            compliance_maps: vec![],
        }],
        scoring: ScoringConfig::default(),
    }
}

/// A short load of `tool`.
fn short_load(tool: LoadTool) -> LoadConfig {
    LoadConfig {
        tool,
        script: std::path::PathBuf::from("http://127.0.0.1:9/health"),
        vus: Some(1),
        duration_s: Some(0.2),
        thresholds: HashMap::new(),
        profile: None,
    }
}

/// Run `gameday` with its one experiment; the outcome and the number of
/// steps the experiment ran.
fn run_one(
    gameday: &crate::types::GameDay,
    config: &RunConfig,
) -> (Result<GameDayJournal, RunnerError>, usize) {
    let mock = MockExecutor::always_succeed();
    let calls = mock.call_count.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
    let result = run_gameday(
        gameday,
        &[minimal_experiment()],
        &executor,
        &Arc::new(ControlRegistry::new()),
        config,
    );
    (result, calls.load(Ordering::SeqCst))
}

#[test]
fn a_game_day_whose_load_does_not_start_ends_with_the_error_before_any_step() {
    let config = RunConfig {
        load_executor: Some(Arc::new(super::load_tests::NoStart)),
        ..RunConfig::default()
    };

    let (result, steps) = run_one(&gameday_under(Some(short_load(LoadTool::Oha))), &config);

    assert!(
        matches!(&result, Err(RunnerError::LoadStart(reason)) if reason == super::load_tests::NO_START),
        "{result:?}"
    );
    assert_eq!(steps, 0);
}

#[test]
fn a_game_day_whose_load_tool_has_no_adapter_ends_with_the_error_before_any_step() {
    let gameday = gameday_under(Some(short_load(LoadTool::Jmeter)));

    let (result, steps) = run_one(&gameday, &RunConfig::default());

    assert!(
        matches!(&result, Err(RunnerError::LoadStart(reason)) if reason.contains("jmeter")),
        "{result:?}"
    );
    assert_eq!(steps, 0);
}

#[test]
fn a_game_day_that_declares_no_load_scores_full_load_impact() {
    let (result, _) = run_one(&gameday_under(None), &RunConfig::default());

    let journal = result.expect("the game day runs");
    assert!(journal.load_result.is_none());
    assert!(
        (journal.resilience_score.load_impact_tolerance - 1.0).abs() < f64::EPSILON,
        "{:?}",
        journal.resilience_score
    );
}

#[test]
fn a_game_day_whose_load_left_no_result_scores_no_load_impact() {
    let (load, _) = super::load_tests::ScriptedLoad::answering(Err("no summary".into()));
    let config = RunConfig {
        load_executor: Some(Arc::new(load)),
        ..RunConfig::default()
    };

    let (result, _) = run_one(&gameday_under(Some(short_load(LoadTool::Oha))), &config);

    let journal = result.expect("the game day runs");
    assert!(journal.load_result.is_none());
    assert!(
        journal.resilience_score.load_impact_tolerance.abs() < f64::EPSILON,
        "{:?}",
        journal.resilience_score
    );
}

#[test]
fn a_game_day_scores_its_load_by_the_share_of_requests_without_error() {
    let measured = LoadResult {
        error_rate: 0.25,
        ..super::load_tests::load_result(100, true)
    };
    let (load, _) = super::load_tests::ScriptedLoad::answering(Ok(measured));
    let config = RunConfig {
        load_executor: Some(Arc::new(load)),
        ..RunConfig::default()
    };

    let (result, _) = run_one(&gameday_under(Some(short_load(LoadTool::Oha))), &config);

    let journal = result.expect("the game day runs");
    assert!(
        (journal.resilience_score.load_impact_tolerance - 0.75).abs() < 1e-9,
        "{:?}",
        journal.resilience_score
    );
}
