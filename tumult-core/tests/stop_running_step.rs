//! A halt or a stop reaches the step that is running.
//!
//! The runner gives the steps of the method to `execute_until`, with the
//! token that a guard or a stop cancels. Rollbacks, guards and the probes of
//! the steady state go through `execute`: a stop must not end the cleanup.
//!
//! The executor here is scripted, so these tests prove the contract between
//! the runner and an executor. That a real process dies is proven with the
//! real executor in `tumult-exec/tests/process_provider.rs` and with the
//! binary in `tumult-cli/tests/run_endings.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// The longest the step named `long` runs when nothing stops it.
const LONG_STEP: Duration = Duration::from_secs(20);

/// The error of a step that was stopped.
const STOPPED: &str = "stopped before it finished";

/// An executor whose step `long` runs until it is stopped. It records which
/// entry point each step came through, and its probes print `down` once the
/// long step runs.
#[derive(Default)]
struct Stoppable {
    calls: Mutex<Vec<(String, &'static str)>>,
    long_step_runs: AtomicBool,
}

impl Stoppable {
    fn record(&self, activity: &Activity, entry: &'static str) {
        self.calls
            .lock()
            .expect("calls")
            .push((activity.name.clone(), entry));
    }

    /// The entry points the step `name` came through, without repeats.
    fn entries(&self, name: &str) -> Vec<&'static str> {
        let mut entries: Vec<_> = self
            .calls
            .lock()
            .expect("calls")
            .iter()
            .filter(|(step, _)| step == name)
            .map(|(_, entry)| *entry)
            .collect();
        entries.dedup();
        entries
    }

    fn wait_for_the_long_step(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.long_step_runs.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the long step never ran");
            // sleep: interval of the bounded poll for the flag above.
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn outcome(success: bool, output: &str, error: Option<&str>) -> ActivityOutcome {
    ActivityOutcome {
        success,
        output: Some(output.into()),
        error: error.map(Into::into),
        duration_ms: 1,
    }
}

impl ActivityExecutor for Stoppable {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.record(activity, "execute");
        let down = activity.name == "watch" && self.long_step_runs.load(Ordering::SeqCst);
        outcome(true, if down { "down" } else { "ok" }, None)
    }

    fn execute_until(&self, activity: &Activity, stop: &CancellationToken) -> ActivityOutcome {
        self.record(activity, "execute_until");
        if activity.name != "long" {
            return outcome(true, "ok", None);
        }
        self.long_step_runs.store(true, Ordering::SeqCst);
        let started = Instant::now();
        while started.elapsed() < LONG_STEP {
            if stop.is_cancelled() {
                return outcome(false, "", Some(STOPPED));
            }
            // sleep: the step runs until the token fires; this is its poll.
            std::thread::sleep(Duration::from_millis(2));
        }
        outcome(true, "ran to its end", None)
    }
}

/// An executor that has no way to stop a step.
struct Plain;

impl ActivityExecutor for Plain {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        outcome(true, &activity.name, None)
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

fn reads_ok(name: &str) -> Activity {
    Activity {
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..activity(name, ActivityType::Probe)
    }
}

/// A long step and one after it, a steady state, and one rollback.
fn experiment() -> Experiment {
    Experiment {
        title: "stop the running step".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![reads_ok("steady")],
        }),
        method: vec![
            activity("long", ActivityType::Action),
            activity("next", ActivityType::Action),
        ],
        rollbacks: vec![activity("undo", ActivityType::Action)],
        ..Experiment::default()
    }
}

fn with_guard() -> Experiment {
    Experiment {
        guards: vec![Guard {
            name: "state-guard".into(),
            probe: reads_ok("watch"),
            min_breaches: 1,
        }],
        ..experiment()
    }
}

fn run(
    experiment: &Experiment,
    target: &Arc<Stoppable>,
    config: &RunConfig,
) -> (Journal, Duration) {
    let executor: Arc<dyn ActivityExecutor> = target.clone();
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
        recovery_interval: Duration::from_millis(10),
    };
    let started = Instant::now();
    let journal = run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        config,
        &sampling,
    )
    .expect("run");
    (journal, started.elapsed())
}

fn result<'a>(journal: &'a Journal, name: &str) -> &'a ActivityResult {
    journal
        .method_results
        .iter()
        .find(|result| result.name == name)
        .unwrap_or_else(|| panic!("the journal has no step {name}"))
}

#[test]
fn a_guard_that_breaks_stops_the_step_that_runs() {
    let target = Arc::new(Stoppable::default());

    let (journal, took) = run(&with_guard(), &target, &RunConfig::default());

    assert_eq!(journal.status, ExperimentStatus::Halted);
    assert!(
        took < LONG_STEP / 2,
        "the halt waited for the step: {took:?}"
    );
    let long = result(&journal, "long");
    assert_eq!(long.status, ActivityStatus::Failed);
    assert_eq!(long.error.as_deref(), Some(STOPPED));
    assert!(target.entries("next").is_empty(), "the method went on");
}

#[test]
fn a_stop_from_outside_stops_the_step_that_runs() {
    let target = Arc::new(Stoppable::default());
    let token = CancellationToken::new();
    let config = RunConfig {
        cancellation_token: Some(token.clone()),
        ..RunConfig::default()
    };
    let watcher = Arc::clone(&target);
    let stopper = std::thread::spawn(move || {
        watcher.wait_for_the_long_step();
        token.cancel();
    });

    let (journal, took) = run(&experiment(), &target, &config);
    stopper.join().expect("the stopper");

    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert!(
        took < LONG_STEP / 2,
        "the stop waited for the step: {took:?}"
    );
    assert_eq!(result(&journal, "long").error.as_deref(), Some(STOPPED));
    assert!(target.entries("next").is_empty(), "the method went on");
}

#[test]
fn only_the_steps_of_the_method_can_be_stopped() {
    let target = Arc::new(Stoppable::default());

    let (journal, _) = run(&with_guard(), &target, &RunConfig::default());

    assert_eq!(target.entries("long"), ["execute_until"]);
    assert_eq!(target.entries("steady"), ["execute"]);
    assert_eq!(target.entries("watch"), ["execute"]);
    assert_eq!(target.entries("undo"), ["execute"]);
    assert_eq!(journal.rollback_results.len(), 1);
    assert_eq!(
        journal.rollback_results[0].status,
        ActivityStatus::Succeeded
    );
}

#[test]
fn an_executor_that_cannot_stop_a_step_runs_it_to_its_end() {
    let stop = CancellationToken::new();
    stop.cancel();

    let outcome = Plain.execute_until(&activity("long", ActivityType::Action), &stop);

    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("long"));
}
