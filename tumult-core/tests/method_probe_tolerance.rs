//! A probe is judged against its tolerance wherever it runs.
//!
//! A probe in the method that reads a value outside its tolerance is a failed
//! step, as it is in the hypothesis. The fault was applied and the target did
//! not behave as the definition says, so the run is deviated, not failed: its
//! rollbacks run and its recovery is measured.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::Status;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_test_utils::{capture_spans, span_attribute};

const OUTSIDE: &str = "the output is outside the tolerance";

/// What a step does when it runs.
#[derive(Clone, Copy)]
enum Step {
    Prints(&'static str),
    CannotRun,
}

/// A target that answers each step by its name. A step it does not know
/// prints `ok`.
struct Target(HashMap<&'static str, Step>);

impl Target {
    fn healthy() -> Self {
        Self(HashMap::new())
    }

    fn with(mut self, name: &'static str, step: Step) -> Self {
        self.0.insert(name, step);
        self
    }
}

impl ActivityExecutor for Target {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let step = self.0.get(activity.name.as_str()).copied();
        let (success, output, error) = match step.unwrap_or(Step::Prints("ok")) {
            Step::Prints(text) => (true, Some(text.into()), None),
            Step::CannotRun => (false, None, Some("exit code 127".into())),
        };
        ActivityOutcome {
            success,
            output,
            error,
            duration_ms: 1,
        }
    }
}

fn activity(name: &str, activity_type: ActivityType) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Native {
            plugin: "test".into(),
            function: "noop".into(),
            arguments: HashMap::new(),
        },
        ..Default::default()
    }
}

/// A probe that holds while the target prints `ok`.
fn probe(name: &str) -> Activity {
    Activity {
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..activity(name, ActivityType::Probe)
    }
}

/// A fault, then a probe of the target while the fault is in, and a rollback.
fn experiment() -> Experiment {
    Experiment {
        title: "probe in the method".into(),
        method: vec![activity("inject", ActivityType::Action), probe("during")],
        rollbacks: vec![activity("restore", ActivityType::Action)],
        ..Experiment::default()
    }
}

/// The same, with a steady state that is checked before and after.
fn with_hypothesis() -> Experiment {
    Experiment {
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![probe("steady")],
        }),
        rto_target_s: Some(30.0),
        ..experiment()
    }
}

fn run(experiment: &Experiment, target: Target) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(target);
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
    };
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("run")
}

fn step<'a>(results: &'a [ActivityResult], name: &str) -> &'a ActivityResult {
    results
        .iter()
        .find(|result| result.name == name)
        .unwrap_or_else(|| panic!("no step {name}"))
}

#[test]
fn a_method_probe_outside_its_tolerance_is_a_failed_step() {
    let target = Target::healthy().with("during", Step::Prints("down"));

    let journal = run(&experiment(), target);

    let during = step(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Failed);
    assert_eq!(during.output.as_deref(), Some("down"));
    assert_eq!(during.error.as_deref(), Some(OUTSIDE));
}

#[test]
fn a_method_probe_within_its_tolerance_is_a_step_that_succeeded() {
    let journal = run(&experiment(), Target::healthy());

    let during = step(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Succeeded);
    assert_eq!(during.error, None);
    assert_eq!(journal.status, ExperimentStatus::Completed);
}

#[test]
fn a_method_probe_outside_its_tolerance_makes_the_run_deviated_and_rolls_back() {
    let target = Target::healthy().with("during", Step::Prints("down"));

    let journal = run(&experiment(), target);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_eq!(journal.rollback_results.len(), 1);
}

#[test]
fn a_method_probe_that_cannot_run_makes_the_run_deviated() {
    let target = Target::healthy().with("during", Step::CannotRun);

    let journal = run(&experiment(), target);

    let during = step(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Failed);
    assert_eq!(during.error.as_deref(), Some("exit code 127"));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
}

#[test]
fn a_rollback_probe_outside_its_tolerance_is_a_failed_rollback() {
    let experiment = Experiment {
        rollbacks: vec![activity("restore", ActivityType::Action), probe("restored")],
        ..experiment()
    };
    let target = Target::healthy()
        .with("during", Step::Prints("down"))
        .with("restored", Step::Prints("down"));

    let journal = run(&experiment, target);

    let restored = step(&journal.rollback_results, "restored");
    assert_eq!(restored.status, ActivityStatus::Failed);
    assert_eq!(restored.error.as_deref(), Some(OUTSIDE));
    assert_eq!(journal.rollback_failures, 1);
}

#[test]
fn a_failed_action_still_makes_the_run_failed() {
    let target = Target::healthy()
        .with("inject", Step::CannotRun)
        .with("during", Step::Prints("down"));

    let journal = run(&experiment(), target);

    assert_eq!(journal.status, ExperimentStatus::Failed);
}

#[test]
fn a_run_with_a_method_probe_outside_its_tolerance_still_measures_recovery() {
    let target = Target::healthy().with("during", Step::Prints("down"));

    let journal = run(&with_hypothesis(), target);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_eq!(journal.hypothesis_met(), Some(true));
    assert!(journal.recovery_time_s().is_some());
    let objectives = journal.objectives.expect("the run has a recovery target");
    assert_eq!(objectives.verdicts[0].nfr_id, "rto");
    assert_eq!(objectives.verdicts[0].state, NfrState::Met);
}

#[test]
fn a_hypothesis_probe_outside_its_tolerance_is_a_failed_step() {
    let target = Target::healthy().with("steady", Step::Prints("down"));

    let journal = run(&with_hypothesis(), target);

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    let before = journal.steady_state_before.expect("checked before");
    assert!(!before.met);
    let steady = step(&before.probe_results, "steady");
    assert_eq!(steady.status, ActivityStatus::Failed);
    assert_eq!(steady.error.as_deref(), Some(OUTSIDE));
}

#[test]
fn the_span_of_a_probe_outside_its_tolerance_has_status_error() {
    let target = Target::healthy().with("during", Step::Prints("down"));

    let (_, spans) = capture_spans(|| run(&experiment(), target));

    let during = spans
        .iter()
        .find(|span| {
            span.name == "resilience.probe"
                && span_attribute(span, "resilience.action.name")
                    .is_some_and(|name| name.as_str() == "during")
        })
        .expect("the probe has a span");
    assert_eq!(during.status, Status::error(OUTSIDE));
}
