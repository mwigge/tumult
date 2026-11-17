//! Every run that ends logs `experiment.completed` once, with its status in
//! the spelling of the journal.

use super::log_capture::{logged, Logged};
use super::*;
use crate::controls::ControlRegistry;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;

/// A probe prints `before` until an action has run and `after` from then
/// on. An action fails when `action_fails` and cancels `cancel`, when given.
struct Scripted {
    before: String,
    after: String,
    acted: AtomicBool,
    action_fails: bool,
    cancel: Option<CancellationToken>,
}

impl Scripted {
    fn answers(before: &str, after: &str) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
            acted: AtomicBool::new(false),
            action_fails: false,
            cancel: None,
        }
    }
}

impl ActivityExecutor for Scripted {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        if activity.activity_type == ActivityType::Action {
            self.acted.store(true, Ordering::SeqCst);
            if let Some(token) = &self.cancel {
                token.cancel();
            }
        }
        let is_action = activity.activity_type == ActivityType::Action;
        let output = if self.acted.load(Ordering::SeqCst) {
            &self.after
        } else {
            &self.before
        };
        ActivityOutcome {
            success: !(is_action && self.action_fails),
            output: Some(output.clone()),
            error: None,
            duration_ms: 1,
        }
    }
}

/// Run the experiment with a hypothesis against `target`; the journal and
/// the `experiment.completed` events the run logged.
fn completed_events(target: Scripted, config: &RunConfig) -> (Journal, Vec<Logged>) {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(target);
    let mut journal = None;
    let events = logged(|| {
        journal = Some(
            run_experiment_with_sampling(
                &experiment_with_hypothesis(),
                &executor,
                &Arc::new(ControlRegistry::new()),
                config,
                &fast_sampling(),
            )
            .expect("run"),
        );
    });
    let completed = events
        .into_iter()
        .filter(|event| event.event == "experiment.completed")
        .collect();
    (journal.expect("the run returned a journal"), completed)
}

/// The run logged one `experiment.completed` event that names the run and
/// carries `status`.
fn assert_logged_once(journal: &Journal, events: &[Logged], status: &str) {
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].field("status"), Some(status));
    assert_eq!(
        events[0].field("experiment_id"),
        Some(journal.experiment_id.as_str())
    );
}

#[test]
fn a_completed_run_logs_completed() {
    let (journal, events) = completed_events(Scripted::answers("200", "200"), &default_config());

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_logged_once(&journal, &events, "completed");
    assert_eq!(events[0].field("deviations"), Some("0"));
}

#[test]
fn a_deviated_run_logs_deviated_and_one_deviation() {
    let (journal, events) = completed_events(Scripted::answers("200", "500"), &default_config());

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_logged_once(&journal, &events, "deviated");
    assert_eq!(events[0].field("deviations"), Some("1"));
}

#[test]
fn a_failed_run_logs_failed() {
    let target = Scripted {
        action_fails: true,
        ..Scripted::answers("200", "200")
    };

    let (journal, events) = completed_events(target, &default_config());

    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert_logged_once(&journal, &events, "failed");
}

#[test]
fn an_aborted_run_logs_aborted() {
    let (journal, events) = completed_events(Scripted::answers("500", "500"), &default_config());

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert_logged_once(&journal, &events, "aborted");
}

#[test]
fn an_interrupted_run_logs_interrupted() {
    let token = CancellationToken::new();
    let config = RunConfig {
        cancellation_token: Some(token.clone()),
        ..default_config()
    };
    let target = Scripted {
        cancel: Some(token),
        ..Scripted::answers("200", "200")
    };

    let (journal, events) = completed_events(target, &config);

    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_logged_once(&journal, &events, "interrupted");
}

#[test]
fn a_run_whose_load_did_not_start_logs_failed_and_why() {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Scripted::answers("200", "200"));
    let config = RunConfig {
        load_executor: Some(Arc::new(super::load_tests::NoStart)),
        ..default_config()
    };
    let mut run = None;

    let events = logged(|| {
        run = Some(run_experiment_with_sampling(
            &super::load_tests::experiment_with_load(),
            &executor,
            &Arc::new(ControlRegistry::new()),
            &config,
            &fast_sampling(),
        ));
    });

    assert!(
        matches!(run, Some(Err(RunnerError::LoadStart(_)))),
        "{run:?}"
    );
    let completed: Vec<_> = events
        .iter()
        .filter(|event| event.event == "experiment.completed")
        .collect();
    assert_eq!(completed.len(), 1, "{events:?}");
    assert_eq!(completed[0].field("status"), Some("failed"));
    let refused = events
        .iter()
        .find(|event| event.event == "failed to start load test")
        .unwrap_or_else(|| panic!("the reason is not logged: {events:?}"));
    assert_eq!(refused.field("error"), Some(super::load_tests::NO_START));
}
