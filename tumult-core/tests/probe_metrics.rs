//! A probe is counted by how it ended, and only a probe its plugin could
//! not run is an error of that plugin. A probe that ran and read a value
//! outside its tolerance found what it was there to find.
//!
//! The metrics bind to the meter provider that is global when the runner
//! first records, so the tests of this binary share one capture and each
//! reads what its own run added, one run at a time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_test_utils::MetricCapture;

const PROBES: &str = "tumult.probes.total";
const PLUGIN_ERRORS: &str = "tumult.plugin.errors.total";
const OUTCOME: &str = "resilience.outcome.status";

/// A target whose probes read `ok` until an action ran. From then on they
/// read `after`, or cannot be run at all when there is none.
struct Target {
    after: Option<&'static str>,
    acted: AtomicBool,
}

impl Target {
    fn after_the_fault(after: Option<&'static str>) -> Self {
        Self {
            after,
            acted: AtomicBool::new(false),
        }
    }
}

impl ActivityExecutor for Target {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        if activity.activity_type == ActivityType::Action {
            self.acted.store(true, Ordering::SeqCst);
            return outcome(Some("done"));
        }
        if self.acted.load(Ordering::SeqCst) {
            return outcome(self.after);
        }
        outcome(Some("ok"))
    }
}

/// A step that printed `output`, or one its plugin could not run.
fn outcome(output: Option<&str>) -> ActivityOutcome {
    ActivityOutcome {
        success: output.is_some(),
        output: output.map(str::to_string),
        error: output.is_none().then(|| "connection refused".to_string()),
        duration_ms: 1,
    }
}

fn step(name: &str, activity_type: ActivityType) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Native {
            plugin: "test".into(),
            function: "noop".into(),
            arguments: std::collections::HashMap::new(),
        },
        ..Default::default()
    }
}

/// A probe that passes only when the target prints `ok`.
fn expects_ok(name: &str) -> Activity {
    Activity {
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..step(name, ActivityType::Probe)
    }
}

/// A fault and a probe after it.
fn fault_then_check() -> Experiment {
    Experiment {
        title: "probe metrics".into(),
        method: vec![step("fault", ActivityType::Action), expects_ok("check")],
        ..Default::default()
    }
}

/// A fault under a steady state that is watched before, during and after.
fn fault_under_watch() -> Experiment {
    Experiment {
        title: "probe metrics".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![expects_ok("steady")],
        }),
        method: vec![step("fault", ActivityType::Action)],
        ..Default::default()
    }
}

/// Probes counted as passed, probes counted as failed, and plugin errors.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Counted {
    passed: f64,
    failed: f64,
    plugin_errors: f64,
}

impl Counted {
    fn so_far() -> Self {
        let capture = MetricCapture::global();
        Self {
            passed: capture.total(PROBES, &[(OUTCOME, "success")]),
            failed: capture.total(PROBES, &[(OUTCOME, "failure")]),
            plugin_errors: capture.total(PLUGIN_ERRORS, &[]),
        }
    }

    fn since(self, before: Self) -> Self {
        Self {
            passed: self.passed - before.passed,
            failed: self.failed - before.failed,
            plugin_errors: self.plugin_errors - before.plugin_errors,
        }
    }
}

/// Run `experiment` against `target`; how the run ended and what it added
/// to the probe metrics.
fn run(experiment: &Experiment, target: Target) -> (ExperimentStatus, Counted) {
    let (journal, added) = journal_of(experiment, target);
    (journal.status, added)
}

/// Run `experiment` against `target`; its journal and what the run added
/// to the probe metrics.
fn journal_of(experiment: &Experiment, target: Target) -> (Journal, Counted) {
    static ONE_RUN: Mutex<()> = Mutex::new(());
    let _one_run = ONE_RUN.lock().unwrap_or_else(PoisonError::into_inner);
    let before = Counted::so_far();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(target);
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
    };
    let journal = run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("run");
    (journal, Counted::so_far().since(before))
}

/// How many probes of a run the journal has as passed, and as failed: the
/// checks of the steady state before and after, and every sample.
fn probes_in(journal: &Journal) -> (f64, f64) {
    let checks = [&journal.steady_state_before, &journal.steady_state_after];
    let checked = checks
        .into_iter()
        .flatten()
        .flat_map(|check| &check.probe_results)
        .map(|probe| probe.status == ActivityStatus::Succeeded);
    let sampled = journal.probe_samples.iter().map(|sample| sample.ok);
    let all: Vec<bool> = checked.chain(sampled).collect();
    let passed = all.iter().filter(|ok| **ok).count();
    (
        f64::from(u32::try_from(passed).unwrap()),
        f64::from(u32::try_from(all.len() - passed).unwrap()),
    )
}

#[test]
fn a_probe_that_read_what_it_expects_is_a_success() {
    let (status, added) = run(&fault_then_check(), Target::after_the_fault(Some("ok")));

    assert_eq!(status, ExperimentStatus::Completed);
    assert_eq!(
        added,
        Counted {
            passed: 1.0,
            failed: 0.0,
            plugin_errors: 0.0
        }
    );
}

#[test]
fn a_probe_that_read_a_value_outside_its_tolerance_failed_and_its_plugin_did_not() {
    let (status, added) = run(&fault_then_check(), Target::after_the_fault(Some("down")));

    assert_eq!(status, ExperimentStatus::Deviated);
    assert_eq!(
        added,
        Counted {
            passed: 0.0,
            failed: 1.0,
            plugin_errors: 0.0
        }
    );
}

#[test]
fn a_probe_its_plugin_could_not_run_is_an_error_of_that_plugin() {
    let (_, added) = run(&fault_then_check(), Target::after_the_fault(None));

    assert_eq!(
        added,
        Counted {
            passed: 0.0,
            failed: 1.0,
            plugin_errors: 1.0
        }
    );
}

#[test]
fn a_run_that_found_its_target_down_counts_no_plugin_error() {
    let (status, added) = run(&fault_under_watch(), Target::after_the_fault(Some("down")));

    assert_eq!(
        (status, added.plugin_errors),
        (ExperimentStatus::Deviated, 0.0)
    );
    assert!(added.failed >= 1.0, "{added:?}");
}

#[test]
fn a_sample_its_plugin_could_not_take_is_an_error_of_that_plugin() {
    let (_, added) = run(&fault_under_watch(), Target::after_the_fault(None));

    // The probe after the fault is one error; every sample adds one more.
    assert!(added.plugin_errors > 1.0, "{added:?}");
    assert!(
        (added.plugin_errors - added.failed).abs() < f64::EPSILON,
        "{added:?}"
    );
}

#[test]
fn a_sample_that_read_a_value_outside_its_tolerance_failed_and_its_plugin_did_not() {
    let (journal, added) = journal_of(&fault_under_watch(), Target::after_the_fault(Some("down")));

    let (passed, failed) = probes_in(&journal);
    // The check after the fault and at least one sample read `down`.
    assert!(failed >= 2.0, "{journal:?}");
    assert_eq!(
        added,
        Counted {
            passed,
            failed,
            plugin_errors: 0.0
        }
    );
}
