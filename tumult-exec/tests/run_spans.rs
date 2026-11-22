//! The spans of a run that the real executor made.
//!
//! The runner drives [`ProviderExecutor`], and every step is a real `sh`
//! command that reads or writes a state file. The spans are the ones a user
//! of `tumult run` exports: the root span says how the run ended, and the
//! span of each step says whether the step held.
#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::Status;
use opentelemetry::Value;
use opentelemetry_sdk::trace::SpanData;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, RunConfig, SamplingConfig,
};
use tumult_core::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, Hypothesis, Journal, Provider, Tolerance,
};
use tumult_exec::ProviderExecutor;
use tumult_test_utils::{capture_spans, span_attribute};

const STATUS: &str = "resilience.outcome.status";
const HYPOTHESIS_MET: &str = "resilience.outcome.hypothesis_met";
const RECOVERY_TIME_S: &str = "resilience.outcome.recovery_time_s";
const BLAST_RADIUS: &str = "resilience.fault.blast_radius";
const OUTSIDE: &str = "the output is outside the tolerance";
const PLUGIN: &str = "resilience.plugin.name";

const INJECT: &str = "echo down > \"$STATE\"";
const HEAL: &str = "echo ok > \"$STATE\"";

/// A target that is one file. It holds `ok` while the target is healthy.
struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn with_state(state: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("state"), format!("{state}\n")).expect("state file");
        Self { dir }
    }

    /// A step that runs `command` in `sh`, with the state file in `$STATE`.
    fn step(&self, name: &str, activity_type: ActivityType, command: &str) -> Activity {
        let state = self.dir.path().join("state").display().to_string();
        Activity {
            name: name.into(),
            activity_type,
            provider: Provider::Process {
                path: "sh".into(),
                arguments: vec!["-c".into(), command.into()],
                env: HashMap::from([("STATE".to_string(), state)]),
                timeout_s: Some(10.0),
            },
            ..Activity::default()
        }
    }

    fn action(&self, name: &str, command: &str) -> Activity {
        self.step(name, ActivityType::Action, command)
    }

    /// A probe that reads the state file and expects `expected` in it.
    fn reads(&self, name: &str, expected: &str) -> Activity {
        Activity {
            tolerance: Some(Tolerance::Regex {
                pattern: format!("^{expected}$"),
            }),
            ..self.step(name, ActivityType::Probe, "cat \"$STATE\"")
        }
    }

    /// `method`, and a steady state that holds while the file says `ok`.
    fn experiment(&self, method: Vec<Activity>) -> Experiment {
        Experiment {
            title: "run spans".into(),
            steady_state_hypothesis: Some(Hypothesis {
                title: "the target answers ok".into(),
                probes: vec![self.reads("steady", "ok")],
            }),
            method,
            ..Experiment::default()
        }
    }
}

/// Run `experiment` with the real executor; the journal and the spans.
fn run(experiment: &Experiment) -> (Journal, Vec<SpanData>) {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(ProviderExecutor::new());
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(200),
        recovery_interval: Duration::from_millis(10),
    };
    capture_spans(|| {
        run_experiment_with_sampling(
            experiment,
            &executor,
            &Arc::new(ControlRegistry::new()),
            &RunConfig::default(),
            &sampling,
        )
        .expect("run")
    })
}

fn root(spans: &[SpanData]) -> SpanData {
    spans
        .iter()
        .find(|span| span.name == "resilience.experiment")
        .expect("the run has a root span")
        .clone()
}

/// The span of the step `name`.
fn step_span(spans: &[SpanData], name: &str) -> SpanData {
    spans
        .iter()
        .find(|span| {
            span_attribute(span, "resilience.action.name")
                .is_some_and(|value| value.as_str() == name)
        })
        .unwrap_or_else(|| panic!("no span of the step {name}"))
        .clone()
}

fn text(span: &SpanData, key: &str) -> Option<String> {
    span_attribute(span, key).map(|value| value.as_str().into_owned())
}

fn flag(span: &SpanData, key: &str) -> Option<bool> {
    match span_attribute(span, key)? {
        Value::Bool(flag) => Some(*flag),
        other => panic!("{key} is not a boolean: {other:?}"),
    }
}

fn seconds(span: &SpanData, key: &str) -> Option<f64> {
    match span_attribute(span, key)? {
        Value::F64(seconds) => Some(*seconds),
        other => panic!("{key} is not a float: {other:?}"),
    }
}

#[test]
fn a_target_that_comes_back_says_completed_and_met_with_status_ok() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![lab.action("inject", INJECT), lab.action("heal", HEAL)]);

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let root = root(&spans);
    assert_eq!(text(&root, STATUS).as_deref(), Some("completed"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(true));
    assert_eq!(root.status, Status::Ok);
}

#[test]
fn the_recovery_time_on_the_root_span_is_the_one_the_journal_measured() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![lab.action("inject", INJECT), lab.action("heal", HEAL)]);

    let (journal, spans) = run(&experiment);

    let measured = journal.recovery_time_s();
    assert!(measured.is_some(), "the target was back at once");
    assert_eq!(seconds(&root(&spans), RECOVERY_TIME_S), measured);
}

#[test]
fn a_target_that_stays_down_says_deviated_and_not_met_with_status_error() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![lab.action("inject", INJECT)]);

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let root = root(&spans);
    assert_eq!(text(&root, STATUS).as_deref(), Some("deviated"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(false));
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
    assert_eq!(root.status, Status::error("deviated"));
}

#[test]
fn a_target_that_is_down_before_the_fault_says_aborted_and_runs_no_step() {
    let lab = Lab::with_state("down");
    let experiment = lab.experiment(vec![lab.action("inject", "echo touched > \"$STATE\"")]);

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    let root = root(&spans);
    assert_eq!(text(&root, STATUS).as_deref(), Some("aborted"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(false));
    assert_eq!(root.status, Status::error("aborted"));
    let state = std::fs::read_to_string(lab.dir.path().join("state")).expect("state file");
    assert_eq!(state, "down\n", "the fault was not applied");
}

#[test]
fn an_action_that_exits_non_zero_says_failed_and_measures_no_recovery() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![lab.action("inject", "exit 3")]);

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Failed);
    let root = root(&spans);
    assert_eq!(text(&root, STATUS).as_deref(), Some("failed"));
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
    assert_eq!(root.status, Status::error("failed"));
    let Status::Error { description } = step_span(&spans, "inject").status else {
        panic!("the span of the failed action has no error status");
    };
    assert!(
        description.contains("exited with exit status: 3"),
        "{description}"
    );
}

#[test]
fn the_span_of_a_probe_says_whether_it_read_what_it_expects() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![
        lab.action("inject", INJECT),
        lab.reads("during", "ok"),
        lab.action("heal", HEAL),
        lab.reads("after", "ok"),
    ]);

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let during = step_span(&spans, "during");
    assert_eq!(during.name, "resilience.probe");
    assert_eq!(during.status, Status::error(OUTSIDE));
    assert_eq!(step_span(&spans, "after").status, Status::Unset);
    let inject = step_span(&spans, "inject");
    assert_eq!(inject.name, "resilience.action");
    assert_eq!(inject.status, Status::Unset);
}

#[test]
fn the_declared_blast_radius_is_on_the_root_span() {
    let lab = Lab::with_state("ok");
    let experiment = Experiment {
        blast_radius: Some("one state file".into()),
        ..lab.experiment(vec![lab.action("noop", "true")])
    };

    let (_, spans) = run(&experiment);

    assert_eq!(
        text(&root(&spans), BLAST_RADIUS).as_deref(),
        Some("one state file")
    );
}

#[test]
fn a_run_without_a_steady_state_gives_no_verdict_on_it() {
    let lab = Lab::with_state("ok");
    let experiment = Experiment {
        steady_state_hypothesis: None,
        ..lab.experiment(vec![lab.action("noop", "true")])
    };

    let (journal, spans) = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Completed);
    let root = root(&spans);
    assert_eq!(flag(&root, HYPOTHESIS_MET), None);
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
    assert_eq!(text(&root, BLAST_RADIUS), None);
}

#[test]
fn the_span_of_each_step_names_the_plugin_that_served_it() {
    let lab = Lab::with_state("ok");
    let experiment = lab.experiment(vec![lab.action("inject", INJECT), lab.action("heal", HEAL)]);

    let (_, spans) = run(&experiment);

    // A command is served by the built-in executor, the plugin `process`:
    // the same name the metrics of the step carry.
    assert_eq!(
        text(&step_span(&spans, "inject"), PLUGIN).as_deref(),
        Some("process")
    );
    assert_eq!(
        text(&step_span(&spans, "steady"), PLUGIN).as_deref(),
        Some("process")
    );
}
