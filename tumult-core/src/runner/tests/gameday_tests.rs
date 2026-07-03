//! GameDay runner tests.

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
