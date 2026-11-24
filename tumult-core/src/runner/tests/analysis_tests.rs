//! The analysis of a run: its estimate against the outcome, the recovery
//! time and the data loss the run measured. The post results here are
//! written by hand.

use super::*;
use crate::runner::phases::compute_analysis;

/// The post phase of a run whose probes saw an outage or not, and that
/// ended recovered or not, with the time to recovery `mttr_s`.
fn post(impact_observed: Option<bool>, full_recovery: bool, mttr_s: Option<f64>) -> PostResult {
    PostResult {
        started_at_ns: 1_800_000_000_000_000_000,
        ended_at_ns: 1_800_000_005_000_000_000,
        duration_s: 5.0,
        samples: 5,
        probes: vec![],
        recovery_time_s: 1.0,
        full_recovery,
        mttr_s,
        impact_observed,
        outage_started_at_ns: None,
        recovered_at_ns: None,
    }
}

/// A run whose probes saw no outage.
fn unaffected() -> PostResult {
    post(Some(false), true, None)
}

/// A run whose probes saw an outage and were back after `mttr_s`.
fn recovered(mttr_s: f64) -> PostResult {
    post(Some(true), true, Some(mttr_s))
}

/// A run whose probes saw an outage and were not back when the watch ended.
fn not_recovered() -> PostResult {
    post(Some(true), false, None)
}

fn estimating(outcome: ExpectedOutcome) -> Experiment {
    Experiment {
        estimate: Some(Estimate {
            expected_outcome: outcome,
            expected_recovery_s: None,
            expected_degradation: None,
            expected_data_loss: None,
            confidence: None,
            rationale: None,
            prior_runs: None,
        }),
        ..minimal_experiment()
    }
}

/// The estimate accuracy of a run of `status` with `post`, estimated as
/// `outcome`.
fn accuracy(
    outcome: ExpectedOutcome,
    status: &ExperimentStatus,
    post: Option<&PostResult>,
) -> Option<f64> {
    compute_analysis(&estimating(outcome), status, post, None)
        .expect("an analysis of the estimate")
        .estimate_accuracy
}

const OUTCOMES: [ExpectedOutcome; 3] = [
    ExpectedOutcome::Unaffected,
    ExpectedOutcome::Recovered,
    ExpectedOutcome::Deviated,
];

#[test]
fn each_expected_outcome_scores_one_only_against_the_outcome_the_run_measured() {
    let completed = ExperimentStatus::Completed;
    let deviated = ExperimentStatus::Deviated;
    let runs = [
        (ExpectedOutcome::Unaffected, &completed, unaffected()),
        (ExpectedOutcome::Recovered, &completed, recovered(3.0)),
        (ExpectedOutcome::Deviated, &deviated, not_recovered()),
    ];

    for (actual, status, post) in &runs {
        for expected in OUTCOMES {
            let wanted = if expected == *actual { 1.0 } else { 0.0 };
            assert_eq!(
                accuracy(expected.clone(), status, Some(post)),
                Some(wanted),
                "{expected} against {actual}"
            );
        }
    }
}

#[test]
fn a_completed_run_that_saw_an_outage_and_did_not_recover_deviated() {
    let completed = ExperimentStatus::Completed;

    assert_eq!(
        accuracy(
            ExpectedOutcome::Deviated,
            &completed,
            Some(&not_recovered())
        ),
        Some(1.0)
    );
    assert_eq!(
        accuracy(
            ExpectedOutcome::Recovered,
            &completed,
            Some(&not_recovered())
        ),
        Some(0.0)
    );
}

#[test]
fn a_deviated_run_deviated_even_when_its_probes_saw_no_outage_or_it_recovered() {
    let deviated = ExperimentStatus::Deviated;

    for post in [Some(unaffected()), Some(recovered(3.0)), None] {
        assert_eq!(
            accuracy(ExpectedOutcome::Deviated, &deviated, post.as_ref()),
            Some(1.0),
            "{post:?}"
        );
        assert_eq!(
            accuracy(ExpectedOutcome::Unaffected, &deviated, post.as_ref()),
            Some(0.0),
            "{post:?}"
        );
    }
}

#[test]
fn a_run_that_failed_or_was_stopped_has_no_estimate_accuracy() {
    for status in [
        ExperimentStatus::Failed,
        ExperimentStatus::Halted,
        ExperimentStatus::Interrupted,
        ExperimentStatus::Aborted,
    ] {
        for expected in OUTCOMES {
            let analysis = compute_analysis(
                &estimating(expected.clone()),
                &status,
                Some(&recovered(3.0)),
                None,
            )
            .expect("an analysis of the estimate");
            assert_eq!(analysis.estimate_accuracy, None, "{expected} {status}");
            assert_eq!(analysis.resilience_score, Some(0.0), "{status}");
        }
    }
}

#[test]
fn a_completed_run_with_no_probe_that_watched_for_an_outage_has_no_estimate_accuracy() {
    let completed = ExperimentStatus::Completed;
    // A journal written before 3.0.4 does not say whether an outage was seen.
    let older = post(None, true, Some(3.0));

    for expected in OUTCOMES {
        assert_eq!(accuracy(expected.clone(), &completed, None), None);
        assert_eq!(accuracy(expected, &completed, Some(&older)), None);
    }
}

fn expecting_recovery_in(seconds: Option<f64>) -> Experiment {
    let mut experiment = estimating(ExpectedOutcome::Recovered);
    if let Some(estimate) = experiment.estimate.as_mut() {
        estimate.expected_recovery_s = seconds;
    }
    experiment
}

fn delta(expected_s: Option<f64>, post: Option<&PostResult>) -> Option<f64> {
    compute_analysis(
        &expecting_recovery_in(expected_s),
        &ExperimentStatus::Completed,
        post,
        None,
    )
    .expect("an analysis of the estimate")
    .estimate_recovery_delta_s
}

#[test]
fn the_recovery_delta_is_the_measured_recovery_time_minus_the_expected_one() {
    assert_eq!(delta(Some(2.0), Some(&recovered(3.0))), Some(1.0));
    assert_eq!(delta(Some(5.0), Some(&recovered(3.0))), Some(-2.0));
    assert_eq!(delta(None, Some(&recovered(3.0))), None);
    assert_eq!(delta(Some(2.0), Some(&not_recovered())), None);
    assert_eq!(delta(Some(2.0), Some(&unaffected())), None);
    assert_eq!(delta(Some(2.0), None), None);
}

fn expecting_data_loss(expected: Option<bool>) -> Experiment {
    let mut experiment = estimating(ExpectedOutcome::Recovered);
    if let Some(estimate) = experiment.estimate.as_mut() {
        estimate.expected_data_loss = expected;
    }
    experiment
}

fn data_loss_correct(
    expected: Option<bool>,
    objectives: Option<&ObjectiveOutcome>,
) -> Option<bool> {
    compute_analysis(
        &expecting_data_loss(expected),
        &ExperimentStatus::Completed,
        Some(&recovered(3.0)),
        objectives,
    )
    .expect("an analysis of the estimate")
    .estimate_data_loss_correct
}

fn rpo(measured_s: Option<f64>) -> ObjectiveOutcome {
    ObjectiveOutcome {
        rpo_target_s: Some(5.0),
        rpo_measured_s: measured_s,
        ..ObjectiveOutcome::default()
    }
}

#[test]
fn the_data_loss_estimate_is_right_when_it_says_whether_the_run_measured_lost_data() {
    let lost = rpo(Some(2.0));
    let kept = rpo(Some(0.0));

    assert_eq!(data_loss_correct(Some(true), Some(&lost)), Some(true));
    assert_eq!(data_loss_correct(Some(true), Some(&kept)), Some(false));
    assert_eq!(data_loss_correct(Some(false), Some(&kept)), Some(true));
    assert_eq!(data_loss_correct(Some(false), Some(&lost)), Some(false));
}

#[test]
fn the_data_loss_estimate_is_not_judged_without_a_measured_rpo_or_an_estimate_of_it() {
    assert_eq!(data_loss_correct(Some(true), Some(&rpo(None))), None);
    assert_eq!(data_loss_correct(Some(true), None), None);
    assert_eq!(data_loss_correct(None, Some(&rpo(Some(2.0)))), None);
}
