//! The number a probe reads: a sample keeps it when the tolerance of the
//! probe reads one, and an rpo objective reads the number read back after
//! the recovery.

use super::*;
use crate::controls::ControlRegistry;
use crate::runner::phases::{samples_for_journal, ProbeSamples};

fn probe(name: &str, tolerance: Option<Tolerance>) -> Activity {
    Activity {
        tolerance,
        ..test_probe(name)
    }
}

fn result(name: &str, output: Option<&str>, ok: bool) -> ActivityResult {
    ActivityResult {
        name: name.into(),
        activity_type: ActivityType::Probe,
        status: if ok {
            ActivityStatus::Succeeded
        } else {
            ActivityStatus::Failed
        },
        started_at_ns: 1_000,
        duration_ms: 12,
        output: output.map(str::to_string),
        error: None,
        trace_id: TraceId::default(),
        span_id: SpanId::default(),
    }
}

#[test]
fn a_sample_keeps_the_number_of_a_probe_whose_tolerance_reads_one() {
    let hypothesis = Hypothesis {
        title: "the store answers".into(),
        probes: vec![
            probe("lost", Some(Tolerance::Number)),
            probe("lag", Some(Tolerance::Range { from: 0.0, to: 5.0 })),
            probe(
                "status",
                Some(Tolerance::Regex {
                    pattern: "^2".into(),
                }),
            ),
            probe("up", None),
        ],
    };
    let samples: ProbeSamples = vec![
        ("lost".into(), vec![result("lost", Some("2.0"), true)]),
        // A lag outside its range is still the lag it read.
        ("lag".into(), vec![result("lag", Some("12"), false)]),
        ("status".into(), vec![result("status", Some("200"), true)]),
        ("up".into(), vec![result("up", Some("7"), true)]),
    ];

    let journal = samples_for_journal(SamplePhase::Post, &samples, &hypothesis);

    let readings: Vec<(&str, Option<f64>)> = journal
        .iter()
        .map(|sample| (sample.probe.as_str(), sample.reading))
        .collect();
    assert_eq!(
        readings,
        [
            ("lost", Some(2.0)),
            ("lag", Some(12.0)),
            ("status", None),
            ("up", None)
        ]
    );
}

#[test]
fn a_sample_whose_output_is_no_number_has_no_reading() {
    let hypothesis = Hypothesis {
        title: "the store answers".into(),
        probes: vec![probe("lost", Some(Tolerance::Number))],
    };
    let outputs = [Some("ok"), Some("\"2\""), Some("2 s"), None];
    let samples: ProbeSamples = vec![(
        "lost".into(),
        outputs
            .iter()
            .map(|output| result("lost", *output, false))
            .collect(),
    )];

    let journal = samples_for_journal(SamplePhase::During, &samples, &hypothesis);

    assert_eq!(journal.len(), outputs.len());
    assert!(journal.iter().all(|sample| sample.reading.is_none()));
}

/// A DR run whose steady-state probe prints the seconds of data lost, with
/// an rpo objective of one second on it.
fn rpo_run(threshold: f64) -> Experiment {
    let mut exp = minimal_experiment();
    exp.kind = TestKind::Dr;
    exp.rto_target_s = Some(30.0);
    exp.rpo_target_s = Some(threshold);
    exp.steady_state_hypothesis = Some(Hypothesis {
        title: "the store answers".into(),
        probes: vec![probe("data-lost", Some(Tolerance::Number))],
    });
    exp.nfr = vec![NfrObjective {
        id: "rpo".into(),
        kind: NfrKind::Rpo,
        metric: "probe.data-lost.lost_s".into(),
        operator: NfrOperator::Lte,
        threshold,
        window_s: None,
        criticality: NfrCriticality::Blocking,
        description: None,
    }];
    exp
}

fn run_reading(experiment: &Experiment, output: &str) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output(output));
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &default_config(),
        &fast_sampling(),
    )
    .expect("the run")
}

#[test]
fn a_run_measures_the_rpo_its_probe_read_back_after_the_recovery() {
    let experiment = rpo_run(1.0);
    assert!(crate::engine::validate_experiment(&experiment).is_ok());

    let journal = run_reading(&experiment, "2.0");

    let objectives = journal.objectives.as_ref().expect("an rpo target");
    assert_eq!(objectives.rpo_measured_s, Some(2.0));
    let rpo: Vec<&NfrVerdict> = objectives
        .verdicts
        .iter()
        .filter(|verdict| verdict.kind == NfrKind::Rpo)
        .collect();
    assert_eq!(rpo.len(), 1, "{rpo:?}");
    assert_eq!(rpo[0].state, NfrState::Breached);
    assert_eq!(rpo[0].observed, Some(2.0));
    assert_eq!(rpo[0].margin, Some(-1.0));
    assert_eq!(rpo[0].reason, None);
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(journal
        .probe_samples
        .iter()
        .all(|sample| sample.reading == Some(2.0)));

    let met = run_reading(&rpo_run(3.0), "2.0");
    let objectives = met.objectives.as_ref().expect("an rpo target");
    assert_eq!(objectives.rpo_measured_s, Some(2.0));
    assert_eq!(met.status, ExperimentStatus::Completed);
}

#[test]
fn a_probe_that_prints_no_number_fails_and_measures_no_rpo() {
    let journal = run_reading(&rpo_run(1.0), "ok");

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    let before = journal.steady_state_before.as_ref().expect("checked first");
    assert!(!before.met);
    assert_eq!(
        before.probe_results[0].error.as_deref(),
        Some(super::super::activity::OUTSIDE_TOLERANCE)
    );
}
