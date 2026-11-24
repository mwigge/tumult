//! The estimate of a run is judged against what the run measured: the
//! outcome (unaffected, recovered or deviated), the recovery time and the
//! data lost.
//!
//! The target is a file in a lab directory and the steady state is a probe
//! that checks it is there. The method leaves it alone, takes it away for
//! three seconds, or takes it away for good. For the data lost, a writer
//! step writes markers to a store and a probe reads back how many seconds
//! of them the fault lost, as in the RPO test of the runner.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tumult_core::controls::ControlRegistry;
use tumult_core::engine::validate_experiment;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// Writes 30 markers, one every tenth of a second.
const WRITER: &str = "t0=$(date +%s); i=0; while [ $i -lt 30 ]; do \
     t=$(awk -v t0=$t0 -v i=$i 'BEGIN { printf \"%.1f\", t0 + i / 10 }'); \
     echo $t >> store && echo $t >> ledger; i=$((i + 1)); sleep 0.1; done";

/// Drops the markers of the last two seconds of the ledger from the store.
const LOSE_THE_TAIL: &str = "cut=$(awk 'END { printf \"%.1f\", $1 - 2 }' ledger); \
     awk -v cut=$cut '$1 <= cut + 0.05' store > kept && mv kept store";

/// Prints the seconds of data lost, and fails when the store is away.
const READER: &str = "test -f store || exit 1; \
     awk 'FILENAME == \"ledger\" { if (first == \"\") first = $1; last = $1; next } \
     { kept = $1 } \
     END { if (last == \"\") lost = 0; else if (kept == \"\") lost = last - first; \
     else lost = last - kept; if (lost < 0) lost = 0; printf \"%.1f\\n\", lost }' ledger store";

/// Runs the process of a step in the directory of the lab, as a process
/// step does.
struct InLab(PathBuf);

impl ActivityExecutor for InLab {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let Provider::Process {
            path, arguments, ..
        } = &activity.provider
        else {
            panic!("the steps of this lab are processes");
        };
        let started = Instant::now();
        let mut command = Command::new(path);
        command
            .args(arguments)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = tumult_core::spawn::alone(|| command.spawn())
            .expect("the process starts")
            .wait_with_output()
            .expect("the process ends");
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        ActivityOutcome {
            success: output.status.success(),
            output: (!stdout.is_empty()).then_some(stdout),
            error: (!output.status.success()).then(|| format!("{}", output.status)),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// A step that runs `command` in `sh`.
fn step(name: &str, activity_type: ActivityType, command: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Process {
            path: "sh".into(),
            arguments: vec!["-c".into(), command.into()],
            env: HashMap::new(),
            timeout_s: Some(10.0),
        },
        ..Default::default()
    }
}

/// An estimate of `outcome`, back in `recovery_s` seconds, that says
/// whether data is lost.
fn estimate(
    outcome: ExpectedOutcome,
    recovery_s: Option<f64>,
    data_loss: Option<bool>,
) -> Estimate {
    Estimate {
        expected_outcome: outcome,
        expected_recovery_s: recovery_s,
        expected_degradation: None,
        expected_data_loss: data_loss,
        confidence: None,
        rationale: None,
        prior_runs: None,
    }
}

/// An experiment with `estimate` whose method runs `method` on a target
/// that a probe checks is there.
fn experiment(method: &str, estimate: Estimate) -> Experiment {
    Experiment {
        title: "the target and the estimate".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target is there".into(),
            probes: vec![step("up", ActivityType::Probe, "test -f up")],
        }),
        method: vec![step("the fault", ActivityType::Action, method)],
        estimate: Some(estimate),
        ..Experiment::default()
    }
}

/// A lab whose target, store and ledger are there.
fn lab() -> tempfile::TempDir {
    let lab = tempfile::TempDir::new().expect("lab directory");
    for file in ["up", "store", "ledger"] {
        std::fs::write(lab.path().join(file), "").expect("a file of the lab");
    }
    lab
}

/// Run `experiment` in a new lab, reading the probes every tenth of a
/// second and watching for recovery for at most `window_s` seconds.
fn run(experiment: &Experiment, window_s: f64) -> Journal {
    validate_experiment(experiment).expect("a valid experiment");
    let lab = lab();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.path().to_path_buf()));
    let sampling = SamplingConfig {
        interval: Duration::from_millis(100),
        max_during_samples: 300,
        recovery_timeout: Duration::from_secs_f64(window_s),
        recovery_interval: Duration::from_millis(100),
    };
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("the run")
}

fn analysis(journal: &Journal) -> &AnalysisResult {
    journal
        .analysis
        .as_ref()
        .expect("an analysis of the estimate")
}

#[test]
fn an_unaffected_estimate_of_a_fault_with_no_effect_is_right() {
    let journal = run(
        &experiment(
            "true",
            estimate(ExpectedOutcome::Unaffected, Some(2.0), Some(false)),
        ),
        5.0,
    );

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert_eq!(post.impact_observed, Some(false), "{post:?}");
    let analysis = analysis(&journal);
    assert_eq!(analysis.estimate_accuracy, Some(1.0), "{analysis:?}");
    // A run that saw no outage measured no recovery time, and a run with
    // no objective that reads the data back measured no RPO.
    assert_eq!(analysis.estimate_recovery_delta_s, None);
    assert_eq!(analysis.estimate_data_loss_correct, None);
}

#[test]
fn a_recovered_estimate_of_a_run_back_in_three_seconds_is_right_and_one_second_slower_than_expected(
) {
    let journal = run(
        &experiment(
            "rm -f up; (sleep 3; touch up) >/dev/null 2>&1 &",
            estimate(ExpectedOutcome::Recovered, Some(2.0), None),
        ),
        10.0,
    );

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(post.full_recovery, "{post:?}");
    assert_eq!(post.impact_observed, Some(true), "{post:?}");
    let rto_s = post.mttr_s.expect("a measured recovery time");
    assert!((2.7..3.5).contains(&rto_s), "{rto_s}");
    let analysis = analysis(&journal);
    assert_eq!(analysis.estimate_accuracy, Some(1.0), "{analysis:?}");
    let delta = analysis
        .estimate_recovery_delta_s
        .expect("the recovery against the estimate");
    assert!((delta - (rto_s - 2.0)).abs() < 1e-9, "{delta} and {rto_s}");
    assert!((0.7..1.5).contains(&delta), "{delta}");
}

#[test]
fn a_deviated_estimate_of_a_run_that_never_recovers_is_right() {
    let journal = run(
        &experiment(
            "rm -f up",
            estimate(ExpectedOutcome::Deviated, Some(2.0), None),
        ),
        1.0,
    );

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery, "{post:?}");
    assert_eq!(post.impact_observed, Some(true), "{post:?}");
    let analysis = analysis(&journal);
    assert_eq!(analysis.estimate_accuracy, Some(1.0), "{analysis:?}");
    // A run that did not recover has no recovery time to compare.
    assert_eq!(analysis.estimate_recovery_delta_s, None);
}

#[test]
fn a_run_whose_method_failed_has_no_estimate_accuracy() {
    let journal = run(
        &experiment(
            "exit 3",
            estimate(ExpectedOutcome::Recovered, Some(2.0), None),
        ),
        1.0,
    );

    assert_eq!(journal.status, ExperimentStatus::Failed);
    let analysis = analysis(&journal);
    assert_eq!(analysis.estimate_accuracy, None, "{analysis:?}");
    assert_eq!(analysis.estimate_recovery_delta_s, None);
    // The value of 0 or 1 of the run keeps its meaning.
    assert_eq!(analysis.resilience_score, Some(0.0));
}

/// A DR experiment that writes markers and then runs `fault`, with an RPO
/// of five seconds and an estimate that data is lost.
fn losing_data(fault: &str) -> Experiment {
    Experiment {
        title: "the store loses its newest writes".into(),
        kind: TestKind::Dr,
        rto_target_s: Some(30.0),
        rpo_target_s: Some(5.0),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the store answers".into(),
            probes: vec![Activity {
                tolerance: Some(Tolerance::Number),
                ..step("data-lost", ActivityType::Probe, READER)
            }],
        }),
        method: vec![
            step("write markers", ActivityType::Action, WRITER),
            step("the fault", ActivityType::Action, fault),
        ],
        nfr: vec![NfrObjective {
            id: "rpo".into(),
            kind: NfrKind::Rpo,
            metric: "probe.data-lost.lost_s".into(),
            operator: NfrOperator::Lte,
            threshold: 5.0,
            window_s: None,
            criticality: NfrCriticality::Blocking,
            description: None,
        }],
        estimate: Some(estimate(ExpectedOutcome::Recovered, None, Some(true))),
        ..Experiment::default()
    }
}

fn rpo_measured_s(journal: &Journal) -> Option<f64> {
    journal.objectives.as_ref()?.rpo_measured_s
}

#[test]
fn an_expected_data_loss_is_right_against_a_run_that_lost_two_seconds_and_wrong_against_one_that_lost_none(
) {
    let lost = run(&losing_data(LOSE_THE_TAIL), 5.0);
    let kept = run(&losing_data("true"), 5.0);

    let lost_s = rpo_measured_s(&lost).expect("the data read back lost");
    assert!((1.9..=2.1).contains(&lost_s), "{lost_s}");
    assert_eq!(analysis(&lost).estimate_data_loss_correct, Some(true));
    assert_eq!(rpo_measured_s(&kept), Some(0.0));
    assert_eq!(analysis(&kept).estimate_data_loss_correct, Some(false));

    let dir = tempfile::TempDir::new().expect("a journal directory");
    let path = dir.path().join("journal.toon");
    tumult_core::journal::write_journal(&lost, &path).expect("write the journal");
    let written = std::fs::read_to_string(&path).expect("the journal file");
    assert!(
        written.contains("estimate_data_loss_correct: true"),
        "{written}"
    );
    let read = tumult_core::journal::read_journal(&path).expect("read the journal");
    assert_eq!(read, lost);
}
