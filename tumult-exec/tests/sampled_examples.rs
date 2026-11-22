//! Shipped experiments that change this host, run by the real runner and
//! the real process provider.
//!
//! The runner samples the probes of the steady state while the method runs,
//! at any moment. These tests pick the moments that hurt: a sample between
//! two steps, and a target that ends a moment after the kill returns. Each
//! run must end as the expectations file of its set says.
//!
//! Each test renames what its experiment touches: the target of the process
//! kill example gets a command line of its own, and the scratch area of a
//! Kronika experiment moves into a temp dir. So a test never ends a process
//! that it did not start, and writes only in its own temp dir. The process
//! kill example starts its target with `setsid`, so these tests run on Linux
//! only.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tumult_core::controls::{ControlRegistry, ProviderControl};
use tumult_core::engine::{parse_experiment, validate_experiment};
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::{Activity, Experiment, HypothesisResult, Journal};
use tumult_exec::ProviderExecutor;

const PROCESS_KILL: &str = "examples/process-kill.toon";
const BATCH_POOL: &str = "demo/kronika/experiments/batch-worker-pool.toon";

/// The command line of the target of the process kill example.
const SHIPPED_TARGET: &str = "sleep 300";
/// The scratch area of the Kronika experiments.
const KRONIKA_SCRATCH: &str = "/tmp/kd-demo";
/// The step of the process kill example that ends the target.
const KILL_STEP: &str = "kill-process";
/// How long the late target lives after the kill returns.
const LATE_EXIT: Duration = Duration::from_millis(300);
/// How long the method waits for the sampler to take one more sample.
const SAMPLE_WAIT: Duration = Duration::from_secs(30);

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the workspace root")
        .to_path_buf()
}

/// A shipped experiment, with each `(from, to)` of `renames` applied to its
/// text. Every mention of `from` must be renamed, or the test would touch
/// what the shipped file touches.
fn shipped(file: &str, renames: &[(&str, &str)]) -> Experiment {
    let mut text = std::fs::read_to_string(workspace_root().join(file)).expect("read the file");
    for (from, to) in renames {
        let mentions = text.matches(from).count();
        assert!(mentions > 0, "{file} no longer names {from}");
        text = text.replace(from, to);
        assert_eq!(text.matches(to).count(), mentions, "{file}: rename {from}");
    }
    let experiment = parse_experiment(&text).expect("parse the file");
    validate_experiment(&experiment).expect("validate the file");
    experiment
}

/// The facts the proof gate compares, as the expectations file next to
/// `file` names them.
fn expected(file: &str) -> serde_json::Value {
    let path = workspace_root().join(file);
    let set = path.with_file_name("expectations.json");
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(set).expect("read the expectations"))
            .expect("parse the expectations");
    let name = path.file_name().and_then(|n| n.to_str()).expect("a name");
    let expect = &body["experiments"][name]["expect"];
    serde_json::json!({
        "status": expect["status"],
        "hypothesis_before": expect["hypothesis_before"],
        "hypothesis_after": expect["hypothesis_after"],
        "rollbacks": expect["rollbacks"],
    })
}

/// The same facts, read from a journal.
fn facts(journal: &Journal) -> serde_json::Value {
    let met = |phase: &Option<HypothesisResult>| phase.as_ref().map(|h| h.met);
    serde_json::json!({
        "status": journal.status,
        "hypothesis_before": met(&journal.steady_state_before),
        "hypothesis_after": met(&journal.steady_state_after),
        "rollbacks": journal.rollback_results.len(),
    })
}

/// The steps of the method, each with its status and output.
fn steps(journal: &Journal) -> Vec<String> {
    journal
        .method_results
        .iter()
        .map(|step| {
            let output = step.output.as_deref().unwrap_or("");
            format!("{} {:?} {output}", step.name, step.status)
        })
        .collect()
}

fn assert_ends_as_expected(file: &str, journal: &Journal) {
    assert_eq!(
        facts(journal),
        expected(file),
        "{file}, steps: {:?}",
        steps(journal)
    );
}

/// A command line that no other process on this host has: the shipped
/// target with a fraction of a second that names this test.
fn own_target(tag: u8) -> String {
    format!("{SHIPPED_TARGET}.{}{tag}", std::process::id())
}

/// Ends every process with the command line `target` when it drops, so a
/// failed test leaves nothing behind.
struct Cleanup(String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("pkill")
            .args(["-f", &format!("^{}$", self.0)])
            .status();
    }
}

fn target_runs(target: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("^{target}$")])
        .output()
        .expect("run pgrep")
        .status
        .success()
}

/// What the method does after each of its steps.
enum Schedule {
    /// Wait until the sampler has taken one more sample of the steady state.
    SampleBetweenSteps,
    /// After the kill, start a process with the command line of the target
    /// that ends a moment later: a target that handles the signal late.
    TargetEndsLate { target: String },
}

#[derive(Default)]
struct Samples {
    started: u64,
    finished: u64,
}

/// Runs every step with the real provider and holds the method to a
/// schedule.
struct Scheduled {
    provider: ProviderExecutor,
    schedule: Schedule,
    steady_state: Vec<String>,
    samples: Mutex<Samples>,
    sampled: Condvar,
    /// True when the schedule could not be kept.
    missed: Mutex<bool>,
}

impl Scheduled {
    fn new(experiment: &Experiment, schedule: Schedule) -> Self {
        let steady_state = experiment
            .steady_state_hypothesis
            .iter()
            .flat_map(|hypothesis| hypothesis.probes.iter())
            .map(|probe| probe.name.clone())
            .collect();
        Self {
            provider: ProviderExecutor::new(),
            schedule,
            steady_state,
            samples: Mutex::new(Samples::default()),
            sampled: Condvar::new(),
            missed: Mutex::new(false),
        }
    }

    fn samples(&self) -> MutexGuard<'_, Samples> {
        self.samples.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn miss(&self) {
        *self.missed.lock().unwrap_or_else(PoisonError::into_inner) = true;
    }

    fn missed(&self) -> bool {
        *self.missed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait for a sample that starts after this call and ends.
    fn wait_for_a_new_sample(&self) {
        let samples = self.samples();
        let wanted = samples.started + 1;
        let (samples, wait) = self
            .sampled
            .wait_timeout_while(samples, SAMPLE_WAIT, |s| s.finished < wanted)
            .unwrap_or_else(PoisonError::into_inner);
        drop(samples);
        if wait.timed_out() {
            self.miss();
        }
    }

    fn start_a_late_target(&self, target: &str) {
        let (program, argument) = target.split_once(' ').expect("a command and its argument");
        let Ok(mut late) = Command::new(program).arg(argument).spawn() else {
            self.miss();
            return;
        };
        std::thread::spawn(move || {
            // sleep: the late exit is the fault under test; the target must
            // still run when the check after the kill starts.
            std::thread::sleep(LATE_EXIT);
            let _ = late.kill();
            let _ = late.wait();
        });
    }

    fn after_step(&self, step: &Activity) {
        match &self.schedule {
            Schedule::SampleBetweenSteps => self.wait_for_a_new_sample(),
            Schedule::TargetEndsLate { target } if step.name == KILL_STEP => {
                self.start_a_late_target(target);
            }
            Schedule::TargetEndsLate { .. } => {}
        }
    }
}

impl ActivityExecutor for Scheduled {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let counted = self.steady_state.contains(&activity.name);
        if counted {
            self.samples().started += 1;
        }
        let outcome = self.provider.execute(activity);
        if counted {
            self.samples().finished += 1;
            self.sampled.notify_all();
        }
        outcome
    }

    fn execute_until(&self, activity: &Activity, stop: &CancellationToken) -> ActivityOutcome {
        let outcome = self.provider.execute_until(activity, stop);
        self.after_step(activity);
        outcome
    }
}

/// Run `experiment` as `tumult run` does: its controls share the executor
/// of the run.
fn run(experiment: &Experiment, scheduled: &Arc<Scheduled>) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = scheduled.clone();
    let mut controls = ControlRegistry::new();
    for control in &experiment.controls {
        controls.register(Box::new(ProviderControl::new(
            control.clone(),
            executor.clone(),
        )));
    }
    let sampling = SamplingConfig {
        interval: Duration::from_millis(100),
        ..SamplingConfig::default()
    };
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(controls),
        &RunConfig::default(),
        &sampling,
    )
    .expect("the run starts")
}

#[test]
fn the_process_kill_example_completes_when_a_sample_runs_between_its_steps() {
    let target = own_target(1);
    let _cleanup = Cleanup(target.clone());
    let experiment = shipped(PROCESS_KILL, &[(SHIPPED_TARGET, &target)]);
    let scheduled = Arc::new(Scheduled::new(&experiment, Schedule::SampleBetweenSteps));

    let journal = run(&experiment, &scheduled);

    assert!(
        !scheduled.missed(),
        "the sampler took no sample between steps"
    );
    assert_ends_as_expected(PROCESS_KILL, &journal);
    assert!(!target_runs(&target), "the run leaves no target behind");
}

#[test]
fn the_process_kill_example_completes_when_the_target_ends_after_the_kill() {
    let target = own_target(2);
    let _cleanup = Cleanup(target.clone());
    let experiment = shipped(PROCESS_KILL, &[(SHIPPED_TARGET, &target)]);
    let schedule = Schedule::TargetEndsLate {
        target: target.clone(),
    };
    let scheduled = Arc::new(Scheduled::new(&experiment, schedule));

    let journal = run(&experiment, &scheduled);

    assert!(!scheduled.missed(), "the late target did not start");
    assert!(
        journal.method_results.iter().any(|s| s.name == KILL_STEP),
        "the example has no step {KILL_STEP}"
    );
    assert_ends_as_expected(PROCESS_KILL, &journal);
    assert!(!target_runs(&target), "the run leaves no target behind");
}

#[test]
fn the_batch_pool_experiment_completes_when_a_sample_runs_between_its_steps() {
    let scratch = tempfile::TempDir::new().expect("temp dir");
    let moved = scratch.path().display().to_string();
    let experiment = shipped(BATCH_POOL, &[(KRONIKA_SCRATCH, &moved)]);
    let scheduled = Arc::new(Scheduled::new(&experiment, Schedule::SampleBetweenSteps));

    let journal = run(&experiment, &scheduled);

    assert!(
        !scheduled.missed(),
        "the sampler took no sample between steps"
    );
    assert_ends_as_expected(BATCH_POOL, &journal);
}
