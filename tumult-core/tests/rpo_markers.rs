//! The RPO of a run is the data its probe reads back lost after the
//! recovery.
//!
//! The store under test is a file in a lab directory. A writer step writes a
//! marker every tenth of a second, the time it was written, and appends the
//! same time to a ledger outside the store when the store has it. The fault
//! drops the markers of the last two seconds. A steady-state probe prints
//! the newest time in the ledger minus the newest marker in the store. The
//! steps are portable `sh` and `awk`: a marker is the start second of the
//! writer and its own count of tenths, so no step needs GNU `date`.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

/// A DR experiment that writes markers and then runs `fault`, with an RPO
/// of `rpo_s` seconds.
fn dr(fault: &str, rpo_s: f64) -> Experiment {
    Experiment {
        title: "the store loses its newest writes".into(),
        kind: TestKind::Dr,
        rto_target_s: Some(30.0),
        rpo_target_s: Some(rpo_s),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the store answers".into(),
            probes: vec![Activity {
                tolerance: Some(Tolerance::Number),
                ..step("data-lost", ActivityType::Probe, READER)
            }],
        }),
        method: vec![
            step("write markers", ActivityType::Action, WRITER),
            step("lose the tail", ActivityType::Action, fault),
        ],
        nfr: vec![NfrObjective {
            id: "rpo".into(),
            kind: NfrKind::Rpo,
            metric: "probe.data-lost.lost_s".into(),
            operator: NfrOperator::Lte,
            threshold: rpo_s,
            window_s: None,
            criticality: NfrCriticality::Blocking,
            description: None,
        }],
        ..Experiment::default()
    }
}

/// A lab whose store and ledger are there and empty.
fn lab() -> tempfile::TempDir {
    let lab = tempfile::TempDir::new().expect("lab directory");
    for file in ["store", "ledger"] {
        std::fs::write(lab.path().join(file), "").expect("an empty file");
    }
    lab
}

/// Run `experiment` in `lab`, reading the probes every half second and
/// watching for recovery for at most `window_s` seconds.
fn run(lab: &Path, experiment: &Experiment, window_s: f64) -> Journal {
    validate_experiment(experiment).expect("a valid experiment");
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.to_path_buf()));
    let sampling = SamplingConfig {
        interval: Duration::from_millis(500),
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

fn rpo(journal: &Journal) -> &NfrVerdict {
    journal
        .objectives
        .as_ref()
        .expect("an rpo target")
        .verdicts
        .iter()
        .find(|verdict| verdict.kind == NfrKind::Rpo)
        .expect("a verdict on the rpo")
}

#[test]
fn a_store_that_loses_its_last_two_seconds_measures_an_rpo_of_two_seconds() {
    let lab = lab();

    let journal = run(lab.path(), &dr(LOSE_THE_TAIL, 1.0), 5.0);

    let verdict = rpo(&journal);
    let lost = verdict.observed.expect("the data read back lost");
    assert!((1.9..=2.1).contains(&lost), "{lost}");
    assert_eq!(verdict.state, NfrState::Breached);
    assert_eq!(verdict.metric, "probe.data-lost.lost_s");
    let objectives = journal.objectives.as_ref().expect("an rpo target");
    assert_eq!(objectives.rpo_measured_s, Some(lost));
    assert_eq!(objectives.rpo_target_s, Some(1.0));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let read_back = journal
        .probe_samples
        .iter()
        .rev()
        .find(|sample| sample.phase == SamplePhase::Post)
        .expect("a read after the method");
    assert_eq!(read_back.reading, Some(lost));

    let dir = tempfile::TempDir::new().expect("a journal directory");
    let path = dir.path().join("journal.toon");
    tumult_core::journal::write_journal(&journal, &path).expect("write the journal");
    let written = std::fs::read_to_string(&path).expect("the journal file");
    assert!(written.contains("reading"), "{written}");
    let read = tumult_core::journal::read_journal(&path).expect("read the journal");
    assert_eq!(read, journal);
}

#[test]
fn a_store_that_loses_less_than_its_rpo_meets_it() {
    let lab = lab();

    let journal = run(lab.path(), &dr(LOSE_THE_TAIL, 3.0), 5.0);

    let verdict = rpo(&journal);
    assert_eq!(verdict.state, NfrState::Met, "{verdict:?}");
    assert_eq!(journal.status, ExperimentStatus::Completed);
}

#[test]
fn a_store_that_does_not_come_back_measures_no_rpo() {
    let lab = lab();

    let journal = run(lab.path(), &dr("rm store", 1.0), 1.0);

    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery, "{post:?}");
    let verdict = rpo(&journal);
    assert_eq!(verdict.state, NfrState::Inconclusive);
    assert_eq!(verdict.observed, None);
    let objectives = journal.objectives.as_ref().expect("an rpo target");
    assert_eq!(objectives.rpo_measured_s, None);
}
