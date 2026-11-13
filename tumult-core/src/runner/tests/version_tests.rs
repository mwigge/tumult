//! The version a run records for its target: declared in the definition,
//! or asked of the target's telemetry once, when the run ends.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::Mutex;

/// Answers with a scripted version and remembers every question.
pub(super) struct ScriptedResolver {
    answer: Result<Option<String>, String>,
    pub(super) asked: Mutex<Vec<RunWindow>>,
}

impl ScriptedResolver {
    pub(super) fn answering(version: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            answer: Ok(version.map(str::to_string)),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn failing(error: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: Err(error.into()),
            asked: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn questions(&self) -> Vec<RunWindow> {
        self.asked.lock().unwrap().clone()
    }
}

impl TargetVersionResolver for ScriptedResolver {
    fn resolve(&self, window: &RunWindow) -> Result<Option<String>, String> {
        self.asked.lock().unwrap().push(window.clone());
        self.answer.clone()
    }
}

pub(super) fn checkout(version: Option<&str>) -> ExperimentTarget {
    ExperimentTarget {
        system: "checkout".into(),
        technology: None,
        environment: Some("demo".into()),
        criticality: None,
        version: version.map(str::to_string),
    }
}

pub(super) fn config_with(resolver: &Arc<ScriptedResolver>) -> RunConfig {
    RunConfig {
        version_resolver: Some(resolver.clone()),
        ..RunConfig::default()
    }
}

fn run(experiment: &Experiment, config: &RunConfig) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    run_experiment_with_sampling(experiment, &executor, &controls, config, &fast_sampling())
        .unwrap()
}

#[test]
fn a_declared_version_is_recorded_and_the_telemetry_is_not_asked() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(Some("1.5.0")));
    let resolver = ScriptedResolver::answering(Some("9.9.9"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.0".into(),
            version_source: VersionSource::Declared,
        })
    );
    assert!(resolver.questions().is_empty(), "declared wins unasked");
}

#[test]
fn an_undeclared_version_is_observed_once_when_the_run_ends() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));
    let resolver = ScriptedResolver::answering(Some("1.5.1"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.1".into(),
            version_source: VersionSource::Observed,
        })
    );
    // Asked once, about this run's own window and environment.
    assert_eq!(
        resolver.questions(),
        vec![RunWindow {
            system: "checkout".into(),
            environment: Some("demo".into()),
            started_at_ns: journal.started_at_ns,
            ended_at_ns: journal.ended_at_ns,
        }]
    );
}

#[test]
fn a_run_records_no_version_when_nothing_knows_it() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));

    // Silent telemetry.
    let silent = ScriptedResolver::answering(None);
    assert_eq!(run(&exp, &config_with(&silent)).target_version, None);
    assert_eq!(silent.questions().len(), 1);

    // A blank answer is no answer.
    let blank = ScriptedResolver::answering(Some("  "));
    assert_eq!(run(&exp, &config_with(&blank)).target_version, None);

    // Telemetry that cannot be read costs the version, not the run.
    let broken = ScriptedResolver::failing("lake unreachable");
    let journal = run(&exp, &config_with(&broken));
    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(journal.target_version, None);

    // No resolver at all.
    assert_eq!(run(&exp, &RunConfig::default()).target_version, None);
}

#[test]
fn a_run_without_a_target_never_asks_for_a_version() {
    let exp = minimal_experiment();
    let resolver = ScriptedResolver::answering(Some("1.5.1"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(journal.target_version, None);
    assert!(resolver.questions().is_empty());
}

#[test]
fn a_run_aborted_by_its_hypothesis_still_observes_its_version() {
    let mut exp = experiment_with_hypothesis();
    exp.target = Some(checkout(None));
    let resolver = ScriptedResolver::answering(Some("1.5.1"));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_fail());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &config_with(&resolver),
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert_eq!(resolver.questions().len(), 1);
    assert_eq!(
        journal.target_version.map(|v| v.version_source),
        Some(VersionSource::Observed)
    );
}
