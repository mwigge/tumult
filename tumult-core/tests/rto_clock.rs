//! The RTO of a run runs from the outage its probes saw to the recovery
//! they saw, wherever the method puts the outage.
//!
//! The target is a file in a lab directory and the steady state is a probe
//! that checks it is there. The method takes it away for four seconds:
//! inside the method, or with a process it leaves behind, so that the
//! outage outlives the method. The probes read every tenth of a second, so
//! the clock is that close to the outage.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// How long the target is away, in seconds.
const OUTAGE_S: f64 = 4.0;

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
        ActivityOutcome {
            success: output.status.success(),
            output: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
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

/// A DR experiment with an RTO of 2 s whose method runs `method`.
fn dr(method: &str) -> Experiment {
    Experiment {
        title: "the target is away for a while".into(),
        kind: TestKind::Dr,
        rto_target_s: Some(2.0),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target is there".into(),
            probes: vec![step("up", ActivityType::Probe, "test -f up")],
        }),
        method: vec![step("take the target away", ActivityType::Action, method)],
        ..Experiment::default()
    }
}

/// The outage held inside the method.
fn inside_the_method() -> Experiment {
    dr(&format!("rm -f up; sleep {OUTAGE_S}; touch up"))
}

/// The outage that outlives the method: a process the method leaves
/// behind brings the target back.
fn after_the_method() -> Experiment {
    dr(&format!(
        "rm -f up; (sleep {OUTAGE_S}; touch up) >/dev/null 2>&1 &"
    ))
}

/// A lab whose target is there.
fn lab() -> tempfile::TempDir {
    let lab = tempfile::TempDir::new().expect("lab directory");
    std::fs::write(lab.path().join("up"), "").expect("the target");
    lab
}

/// Run `experiment` in `lab`, reading the probes every tenth of a second
/// and watching for recovery for at most `window_s` seconds.
fn run(lab: &Path, experiment: &Experiment, window_s: f64) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.to_path_buf()));
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

fn rto(journal: &Journal) -> &NfrVerdict {
    journal
        .objectives
        .as_ref()
        .expect("a recovery target")
        .verdicts
        .iter()
        .find(|verdict| verdict.nfr_id == "rto")
        .expect("a verdict on the rto")
}

fn seconds(ns: i64) -> f64 {
    // A run of a few seconds is far below 2^53 nanoseconds.
    #[allow(clippy::cast_precision_loss)]
    let seconds = ns as f64 / 1e9;
    seconds
}

/// The RTO `journal` measured.
fn measured(journal: &Journal) -> f64 {
    let objectives = journal.objectives.as_ref().expect("a recovery target");
    objectives.rto_measured_s.expect("a measured rto")
}

/// Every place of `journal` that holds the RTO says `rto_s`, and the
/// outage and the recovery it was seen between.
fn says_everywhere(journal: &Journal, rto_s: f64) {
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(post.full_recovery, "{post:?}");
    assert_eq!(post.impact_observed, Some(true), "{post:?}");
    let outage = post.outage_started_at_ns.expect("the outage seen");
    let recovered = post.recovered_at_ns.expect("the recovery seen");
    assert!(
        (rto_s - seconds(recovered - outage)).abs() < 1e-6,
        "{post:?}"
    );
    assert_eq!(post.mttr_s, Some(rto_s));
    assert_eq!(journal.recovery_time_s(), Some(rto_s));
    let verdict = rto(journal);
    assert_eq!(verdict.observed, Some(rto_s), "{verdict:?}");
    assert_eq!(verdict.state, NfrState::Breached, "{verdict:?}");
    assert_eq!(verdict.recovered, Some(true));
    assert_eq!(verdict.reason, None);
}

#[test]
fn an_outage_inside_the_method_and_one_that_outlives_it_measure_the_same_rto() {
    let inside_lab = lab();
    let after_lab = lab();

    let inside = run(inside_lab.path(), &inside_the_method(), 10.0);
    let after = run(after_lab.path(), &after_the_method(), 10.0);

    let inside_s = measured(&inside);
    let after_s = measured(&after);
    assert!((3.7..4.5).contains(&inside_s), "inside: {inside_s}");
    assert!((3.7..4.5).contains(&after_s), "after: {after_s}");
    assert!((inside_s - after_s).abs() < 0.4, "{inside_s} and {after_s}");
    says_everywhere(&inside, inside_s);
    says_everywhere(&after, after_s);
    assert_eq!(inside.status, ExperimentStatus::Deviated);
    assert_eq!(after.status, ExperimentStatus::Deviated);
    // The time from the end of the method keeps its meaning: the target
    // was back when the method ended.
    let post = inside.post_result.as_ref().expect("the run watched");
    assert!(post.recovery_time_s < 0.5, "{post:?}");
}

#[test]
fn a_fault_with_no_visible_effect_has_an_inconclusive_rto_and_no_recovery() {
    let lab = lab();

    let journal = run(lab.path(), &dr("true"), 10.0);

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(post.full_recovery, "{post:?}");
    assert_eq!(post.impact_observed, Some(false));
    assert_eq!(post.outage_started_at_ns, None);
    assert_eq!(post.recovered_at_ns, None);
    assert_eq!(post.mttr_s, None);
    assert_eq!(journal.recovery_time_s(), None);
    let objectives = journal.objectives.as_ref().expect("a recovery target");
    assert_eq!(objectives.rto_measured_s, None);
    let verdict = rto(&journal);
    assert_eq!(verdict.state, NfrState::Inconclusive);
    assert_eq!(
        verdict.reason.as_deref(),
        Some(NfrVerdict::NO_OUTAGE_OBSERVED)
    );
    assert_eq!(verdict.recovered, None);
    let dir = tempfile::TempDir::new().expect("a journal directory");
    let path = dir.path().join("journal.toon");
    tumult_core::journal::write_journal(&journal, &path).expect("write the journal");
    let written = std::fs::read_to_string(&path).expect("the journal file");
    assert!(written.contains("impact_observed: false"), "{written}");
    assert!(written.contains("no outage observed"), "{written}");
    let read = tumult_core::journal::read_journal(&path).expect("read the journal");
    assert_eq!(read, journal);
}

#[test]
fn a_run_that_never_recovers_breaches_its_rto_from_the_outage_seen_to_the_end_of_the_watch() {
    let lab = lab();

    // The target goes away one second before the method ends and never
    // comes back; the watch is one second.
    let journal = run(lab.path(), &dr("rm -f up; sleep 1"), 1.0);

    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery, "{post:?}");
    assert_eq!(post.impact_observed, Some(true));
    assert!(post.outage_started_at_ns.is_some(), "{post:?}");
    assert_eq!(post.recovered_at_ns, None);
    let verdict = rto(&journal);
    assert_eq!(verdict.state, NfrState::Breached);
    assert_eq!(verdict.recovered, Some(false));
    let watched = verdict.observed.expect("a lower bound");
    // The method second counts, as well as the watch after it.
    assert!(
        watched > post.recovery_time_s + 0.7,
        "{watched} against {post:?}"
    );
    assert!((1.6..2.2).contains(&watched), "{watched}");
}
