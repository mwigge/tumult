//! The recovery window of a run is set by its experiment.
//!
//! The target is a file in a lab directory. The method removes it, and a
//! process it leaves behind puts it back after a number of seconds, so the
//! outage outlives the method and the time it takes comes from the clock.
//! The steady state is a probe that checks the file is there. The numbers
//! are short: a two second outage stands for the forty second one of the
//! live proof, with the window above or below it.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, run_gameday, ActivityExecutor, ActivityOutcome};
use tumult_core::runner::{RunConfig, RunnerError};
use tumult_core::types::*;

/// How long the target stays down after the method, in seconds.
const OUTAGE_S: f64 = 2.0;

/// How far a watch may run past its window: the round of reads that is
/// running when the window ends finishes, and a slow runner (macOS CI)
/// takes about a fifth of a second for one read of the probe.
const ROUND_S: f64 = 0.5;

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

/// A DR experiment whose target is down for [`OUTAGE_S`] after the method,
/// with an RTO of `rto_target_s` and the recovery block `recovery`.
fn outage(rto_target_s: f64, recovery: RecoveryConfig) -> Experiment {
    let restore = format!("rm -f up; (sleep {OUTAGE_S}; touch up) >/dev/null 2>&1 &");
    Experiment {
        title: "the target comes back after an outage".into(),
        kind: TestKind::Dr,
        rto_target_s: Some(rto_target_s),
        recovery: Some(recovery),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target is there".into(),
            probes: vec![step("up", ActivityType::Probe, "test -f up")],
        }),
        method: vec![step("remove the target", ActivityType::Action, &restore)],
        ..Experiment::default()
    }
}

/// A window of `window_s` seconds, read every tenth of a second.
fn window(window_s: f64) -> RecoveryConfig {
    RecoveryConfig {
        window_s,
        interval_s: 0.1,
    }
}

/// A lab whose target is there.
fn lab() -> tempfile::TempDir {
    let lab = tempfile::TempDir::new().expect("lab directory");
    std::fs::write(lab.path().join("up"), "").expect("the target");
    lab
}

fn run(lab: &Path, experiment: &Experiment) -> Result<Journal, RunnerError> {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.to_path_buf()));
    run_experiment(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
    )
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

#[test]
fn an_outage_shorter_than_the_window_meets_an_rto_the_window_can_watch() {
    let lab = lab();
    let experiment = outage(3.0, window(3.0));

    let journal = run(lab.path(), &experiment).expect("the run");

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(post.full_recovery, "{post:?}");
    let verdict = rto(&journal);
    assert_eq!(verdict.state, NfrState::Met, "{verdict:?}");
    assert_eq!(verdict.recovered, Some(true));
    let measured = verdict.observed.expect("a recovery time");
    assert!((1.8..2.8).contains(&measured), "{measured}");
    // The watch stops at recovery, before the end of the window, and reads
    // the target every tenth of a second.
    assert!(post.duration_s < 2.9, "{post:?}");
    assert!(post.samples >= 6, "{post:?}");
}

#[test]
fn the_same_outage_breaches_the_rto_when_the_window_ends_first() {
    let lab = lab();
    let experiment = outage(3.0, window(1.0));

    let journal = run(lab.path(), &experiment).expect("the run");

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery, "{post:?}");
    // The watch ends with the window, or with the round that runs then.
    assert!(post.duration_s <= 1.0 + ROUND_S, "{post:?}");
    assert!(post.samples >= 4, "{post:?}");
    let verdict = rto(&journal);
    assert_eq!(verdict.state, NfrState::Breached, "{verdict:?}");
    assert_eq!(verdict.recovered, Some(false));
    let watched = verdict.observed.expect("the time watched");
    assert!(watched <= 1.0 + ROUND_S, "{watched}");
}

#[test]
fn a_game_day_watches_each_run_for_the_window_of_its_experiment() {
    let lab = lab();
    let experiment = outage(3.0, window(1.0));
    let gameday = GameDay {
        title: "Lab game day".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        experiments: vec![GameDayExperiment {
            path: "outage.toon".into(),
            compliance_maps: vec![],
        }],
        scoring: ScoringConfig::default(),
        load: None,
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.path().to_path_buf()));

    let journal = run_gameday(
        &gameday,
        &[experiment],
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
    )
    .expect("the game day runs");

    let run = &journal.experiment_journals[0];
    let post = run.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery, "{post:?}");
    assert!(post.duration_s <= 1.0 + ROUND_S, "{post:?}");
    assert_eq!(rto(run).state, NfrState::Breached);
}

#[test]
fn a_run_refuses_a_window_it_cannot_watch_before_anything_runs() {
    let lab = lab();
    let experiment = outage(3.0, window(3601.0));

    let refused = run(lab.path(), &experiment);

    assert!(
        matches!(&refused, Err(RunnerError::InvalidRecovery(reason)) if reason.contains("3600")),
        "{refused:?}"
    );
    assert!(lab.path().join("up").exists(), "the method ran");
}
