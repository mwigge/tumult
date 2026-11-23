//! The probe metrics of a run that the real executor made.
//!
//! Every step is a real `sh` command. A probe that ran and read a target
//! that is down is no error of its plugin; a probe whose command failed is.
//!
//! The metrics bind to the meter provider that is global when the runner
//! first records, so the tests of this binary share one capture and each
//! reads what its own run added, one run at a time.
#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, RunConfig, SamplingConfig,
};
use tumult_core::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, Provider, Tolerance,
};
use tumult_exec::ProviderExecutor;
use tumult_test_utils::MetricCapture;

const PROBES: &str = "tumult.probes.total";
const PLUGIN_ERRORS: &str = "tumult.plugin.errors.total";
const OUTCOME: &str = "resilience.outcome.status";

/// A probe that runs `command` in `sh` and expects it to print `ok`.
fn expects_ok(command: &str) -> Activity {
    Activity {
        name: "check".into(),
        activity_type: ActivityType::Probe,
        provider: Provider::Process {
            path: "sh".into(),
            arguments: vec!["-c".into(), command.into()],
            env: HashMap::new(),
            timeout_s: Some(10.0),
        },
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..Activity::default()
    }
}

/// Probes passed, probes failed and plugin errors counted so far.
fn counted() -> [f64; 3] {
    let capture = MetricCapture::global();
    [
        capture.total(PROBES, &[(OUTCOME, "success")]),
        capture.total(PROBES, &[(OUTCOME, "failure")]),
        capture.total(PLUGIN_ERRORS, &[]),
    ]
}

/// Run one probe with the real executor; how the run ended, and the probes
/// passed, the probes failed and the plugin errors it added.
fn run(probe: Activity) -> (ExperimentStatus, [f64; 3]) {
    static ONE_RUN: Mutex<()> = Mutex::new(());
    let _one_run = ONE_RUN.lock().unwrap_or_else(PoisonError::into_inner);
    let before = counted();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(ProviderExecutor::new());
    let experiment = Experiment {
        title: "probe metrics".into(),
        method: vec![probe],
        ..Experiment::default()
    };
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(200),
        recovery_interval: Duration::from_millis(10),
    };
    let journal = run_experiment_with_sampling(
        &experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("run");
    let after = counted();
    (
        journal.status,
        [
            after[0] - before[0],
            after[1] - before[1],
            after[2] - before[2],
        ],
    )
}

#[test]
fn a_probe_that_read_what_it_expects_is_a_success() {
    let (status, added) = run(expects_ok("echo ok"));

    assert_eq!(
        (status, added),
        (ExperimentStatus::Completed, [1.0, 0.0, 0.0])
    );
}

#[test]
fn a_probe_that_read_a_target_that_is_down_failed_and_its_plugin_did_not() {
    let (status, added) = run(expects_ok("echo down"));

    assert_eq!(
        (status, added),
        (ExperimentStatus::Deviated, [0.0, 1.0, 0.0])
    );
}

#[test]
fn a_probe_whose_command_failed_is_an_error_of_its_plugin() {
    let (status, added) = run(expects_ok("echo ok; exit 3"));

    assert_eq!(
        (status, added),
        (ExperimentStatus::Deviated, [0.0, 1.0, 1.0])
    );
}
