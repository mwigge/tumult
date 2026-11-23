//! The root span of a run says how the run ended.
//!
//! Reports and the recovery numbers read the outcome of a run from its
//! `resilience.experiment` span: the status in the spelling of the journal,
//! whether the steady state held, the recovery time and the declared blast
//! radius. Only a completed run has span status Ok.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::Status;
use opentelemetry::Value;
use opentelemetry_sdk::trace::SpanData;
use tokio_util::sync::CancellationToken;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_test_utils::{capture_spans, span_attribute};

const STATUS: &str = "resilience.outcome.status";
const HYPOTHESIS_MET: &str = "resilience.outcome.hypothesis_met";
const RECOVERY_TIME_S: &str = "resilience.outcome.recovery_time_s";
const BLAST_RADIUS: &str = "resilience.fault.blast_radius";

/// What the scripted target does: a probe prints `before` until an action
/// has run and `after` from then on. An action cancels `cancel`, when given,
/// and fails when `action_fails`.
struct Target {
    before: &'static str,
    after: &'static str,
    acted: AtomicBool,
    cancel: Option<CancellationToken>,
    action_fails: bool,
}

impl Target {
    fn answers(before: &'static str, after: &'static str) -> Self {
        Self {
            before,
            after,
            acted: AtomicBool::new(false),
            cancel: None,
            action_fails: false,
        }
    }
}

impl ActivityExecutor for Target {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let output = match activity.activity_type {
            ActivityType::Probe if self.acted.load(Ordering::SeqCst) => self.after,
            ActivityType::Probe => self.before,
            ActivityType::Action => {
                self.acted.store(true, Ordering::SeqCst);
                if let Some(token) = &self.cancel {
                    token.cancel();
                }
                "done"
            }
        };
        let failed = self.action_fails && activity.activity_type == ActivityType::Action;
        ActivityOutcome {
            success: !failed,
            output: Some(output.into()),
            error: None,
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
            arguments: std::collections::HashMap::new(),
        },
        ..Default::default()
    }
}

fn one_action(title: &str) -> Experiment {
    Experiment {
        title: title.into(),
        method: vec![activity("action-1", ActivityType::Action)],
        ..Experiment::default()
    }
}

/// One action, and a steady state that holds while the target prints `ok`.
fn with_hypothesis(title: &str) -> Experiment {
    let mut probe = activity("steady", ActivityType::Probe);
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "ok".into(),
    });
    Experiment {
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![probe],
        }),
        ..one_action(title)
    }
}

/// Run `experiment` against `target`; the journal and the root span.
fn run(experiment: &Experiment, target: Target, config: &RunConfig) -> (Journal, SpanData) {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(target);
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
    };
    let (journal, spans) = capture_spans(|| {
        run_experiment_with_sampling(
            experiment,
            &executor,
            &Arc::new(ControlRegistry::new()),
            config,
            &sampling,
        )
        .expect("run")
    });
    let root = spans
        .into_iter()
        .find(|span| span.name == "resilience.experiment")
        .expect("the run has a root span");
    (journal, root)
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
fn a_completed_run_says_completed_and_met_with_status_ok() {
    let (journal, root) = run(
        &with_hypothesis("completed"),
        Target::answers("ok", "ok"),
        &RunConfig::default(),
    );

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(text(&root, STATUS).as_deref(), Some("completed"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(true));
    assert_eq!(root.status, Status::Ok);
}

#[test]
fn a_deviated_run_says_deviated_and_not_met_with_status_error() {
    let (journal, root) = run(
        &with_hypothesis("deviated"),
        Target::answers("ok", "down"),
        &RunConfig::default(),
    );

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_eq!(text(&root, STATUS).as_deref(), Some("deviated"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(false));
    assert_eq!(root.status, Status::error("deviated"));
}

#[test]
fn an_aborted_run_says_aborted_and_not_met_with_status_error() {
    let (journal, root) = run(
        &with_hypothesis("aborted"),
        Target::answers("down", "down"),
        &RunConfig::default(),
    );

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert_eq!(text(&root, STATUS).as_deref(), Some("aborted"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), Some(false));
    assert_eq!(root.status, Status::error("aborted"));
}

#[test]
fn an_interrupted_run_says_interrupted_and_gives_no_verdict_on_the_hypothesis() {
    let token = CancellationToken::new();
    let config = RunConfig {
        cancellation_token: Some(token.clone()),
        ..RunConfig::default()
    };
    let target = Target {
        cancel: Some(token),
        ..Target::answers("ok", "ok")
    };

    let (journal, root) = run(&with_hypothesis("interrupted"), target, &config);

    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(text(&root, STATUS).as_deref(), Some("interrupted"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), None);
    assert_eq!(root.status, Status::error("interrupted"));
}

#[test]
fn a_run_without_a_hypothesis_gives_no_verdict_on_it_and_no_recovery_time() {
    let (journal, root) = run(
        &one_action("no hypothesis"),
        Target::answers("ok", "ok"),
        &RunConfig::default(),
    );

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(text(&root, STATUS).as_deref(), Some("completed"));
    assert_eq!(flag(&root, HYPOTHESIS_MET), None);
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
}

#[test]
fn the_recovery_time_is_the_one_the_journal_measured() {
    let (journal, root) = run(
        &with_hypothesis("recovered"),
        Target::answers("ok", "ok"),
        &RunConfig::default(),
    );

    let measured = journal.recovery_time_s();
    assert!(measured.is_some(), "the target was back at once");
    assert_eq!(seconds(&root, RECOVERY_TIME_S), measured);
}

#[test]
fn a_run_that_did_not_see_recovery_has_no_recovery_time() {
    let (journal, root) = run(
        &with_hypothesis("never back"),
        Target::answers("ok", "down"),
        &RunConfig::default(),
    );

    assert_eq!(journal.recovery_time_s(), None);
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
}

#[test]
fn the_declared_blast_radius_is_on_the_root_span() {
    let experiment = Experiment {
        blast_radius: Some("payments namespace only".into()),
        ..one_action("declared blast radius")
    };

    let (_, root) = run(
        &experiment,
        Target::answers("ok", "ok"),
        &RunConfig::default(),
    );

    assert_eq!(
        text(&root, BLAST_RADIUS).as_deref(),
        Some("payments namespace only")
    );
}

#[test]
fn a_run_that_declares_no_blast_radius_has_none_on_the_root_span() {
    let (_, root) = run(
        &one_action("no blast radius"),
        Target::answers("ok", "ok"),
        &RunConfig::default(),
    );

    assert_eq!(text(&root, BLAST_RADIUS), None);
}

#[test]
fn a_run_whose_method_failed_measures_no_recovery() {
    let experiment = Experiment {
        rto_target_s: Some(30.0),
        ..with_hypothesis("failed method")
    };
    let target = Target {
        action_fails: true,
        ..Target::answers("ok", "ok")
    };

    let (journal, root) = run(&experiment, target, &RunConfig::default());

    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert_eq!(journal.recovery_time_s(), None);
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
    let objectives = journal.objectives.expect("the run has a recovery target");
    assert_eq!(objectives.rto_measured_s, None);
    assert_eq!(objectives.verdicts[0].nfr_id, "rto");
    assert_eq!(objectives.verdicts[0].state, NfrState::Inconclusive);
}

#[test]
fn a_run_that_never_recovered_breaches_its_rto_with_the_time_it_watched() {
    let experiment = Experiment {
        rto_target_s: Some(30.0),
        rpo_target_s: Some(60.0),
        ..with_hypothesis("never back")
    };

    let (journal, root) = run(
        &experiment,
        Target::answers("ok", "down"),
        &RunConfig::default(),
    );

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_eq!(seconds(&root, RECOVERY_TIME_S), None);
    let post = journal.post_result.as_ref().expect("the run watched");
    assert!(!post.full_recovery);
    let objectives = journal.objectives.as_ref().expect("a recovery target");
    assert_eq!(objectives.rto_measured_s, None);
    let rto = &objectives.verdicts[0];
    assert_eq!(rto.nfr_id, "rto");
    assert_eq!(rto.state, NfrState::Breached);
    assert_eq!(rto.observed, Some(post.recovery_time_s));
    assert_eq!(rto.margin, None);
    assert_eq!(rto.recovered, Some(false));
    let dir = tempfile::TempDir::new().expect("a journal directory");
    let path = dir.path().join("journal.toon");
    tumult_core::journal::write_journal(&journal, &path).expect("write the journal");
    let read = tumult_core::journal::read_journal(&path).expect("read the journal");
    assert_eq!(read, journal);
}

#[test]
fn a_run_whose_method_failed_and_never_recovered_measures_no_recovery() {
    let experiment = Experiment {
        rto_target_s: Some(30.0),
        ..with_hypothesis("failed, never back")
    };
    let target = Target {
        action_fails: true,
        ..Target::answers("ok", "down")
    };

    let (journal, _) = run(&experiment, target, &RunConfig::default());

    assert_eq!(journal.status, ExperimentStatus::Failed);
    let objectives = journal.objectives.expect("the run has a recovery target");
    assert_eq!(objectives.verdicts[0].nfr_id, "rto");
    assert_eq!(objectives.verdicts[0].state, NfrState::Inconclusive);
    assert_eq!(objectives.verdicts[0].observed, None);
    assert_eq!(objectives.verdicts[0].recovered, None);
}

/// One action under an oha load whose binary is missing.
fn under_missing_oha(title: &str) -> (Experiment, RunConfig) {
    let experiment = Experiment {
        load: Some(LoadConfig {
            tool: LoadTool::Oha,
            script: "http://127.0.0.1:9/health".into(),
            vus: Some(1),
            duration_s: Some(1.0),
            thresholds: std::collections::HashMap::new(),
            profile: None,
        }),
        ..one_action(title)
    };
    let config = RunConfig {
        load_executor: Some(Arc::new(
            tumult_core::runner::oha::OhaLoadExecutor::default(),
        )),
        ..RunConfig::default()
    };
    (experiment, config)
}

#[test]
fn a_run_whose_load_did_not_start_says_failed_and_its_load_span_says_why() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let tools = tempfile::TempDir::new().expect("tools directory");
    env.set("TUMULT_OHA_BINARY", tools.path().join("no-oha"));
    let (experiment, config) = under_missing_oha("load did not start");
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Target::answers("ok", "ok"));

    let (run, spans) = capture_spans(|| {
        run_experiment_with_sampling(
            &experiment,
            &executor,
            &Arc::new(ControlRegistry::new()),
            &config,
            &SamplingConfig::default(),
        )
    });

    assert!(
        matches!(&run, Err(tumult_core::runner::RunnerError::LoadStart(_))),
        "{run:?}"
    );
    let span = |name: &str| {
        spans
            .iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("no {name} span"))
    };
    let root = span("resilience.experiment");
    assert_eq!(text(root, STATUS).as_deref(), Some("failed"));
    assert_eq!(root.status, Status::error("failed"));
    let load = span("resilience.load");
    assert!(
        matches!(&load.status, Status::Error { description } if description.contains("failed to start oha")),
        "{:?}",
        load.status
    );
    let why = text(load, "resilience.load.error");
    assert!(
        why.as_deref()
            .is_some_and(|why| why.contains("failed to start oha")),
        "{why:?}"
    );
}

#[test]
fn a_game_day_whose_load_did_not_start_says_why_on_its_span_and_runs_nothing() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let tools = tempfile::TempDir::new().expect("tools directory");
    env.set("TUMULT_OHA_BINARY", tools.path().join("no-oha"));
    let (experiment, config) = under_missing_oha("game day load did not start");
    let gameday = GameDay {
        title: "load did not start".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        load: experiment.load.clone(),
        experiments: vec![GameDayExperiment {
            path: "one.toon".into(),
            compliance_maps: vec![],
        }],
        scoring: ScoringConfig::default(),
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Target::answers("ok", "ok"));

    let (run, spans) = capture_spans(|| {
        tumult_core::runner::run_gameday(
            &gameday,
            &[one_action("one")],
            &executor,
            &Arc::new(ControlRegistry::new()),
            &config,
        )
    });

    assert!(
        matches!(&run, Err(tumult_core::runner::RunnerError::LoadStart(_))),
        "{run:?}"
    );
    let campaign = spans
        .iter()
        .find(|span| span.name == "resilience.gameday")
        .expect("the span of the game day");
    assert!(
        matches!(&campaign.status, Status::Error { description } if description.contains("failed to start oha")),
        "{:?}",
        campaign.status
    );
    assert!(spans
        .iter()
        .all(|span| span.name != "resilience.experiment"));
}
