//! The experiment outcome metrics (`tumult.experiments.total`,
//! `tumult.experiment.duration`) are recorded on every way a started run
//! can end: completed, aborted, interrupted and halted. Only a completed
//! run counts as a success.
//!
//! The metrics bind to the meter provider that is global when the runner
//! first records, so this binary installs one provider for all its tests
//! and each test reads what its own run added, one run at a time.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tokio_util::sync::CancellationToken;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// What the scripted target does: every probe prints `probe`, every
/// action takes `action` and, when a token is given, cancels it.
struct Scripted {
    probe: &'static str,
    action: Duration,
    cancel: Option<CancellationToken>,
}

impl ActivityExecutor for Scripted {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let output = match activity.activity_type {
            ActivityType::Probe => self.probe,
            ActivityType::Action => {
                // sleep: how long the scripted action takes is the input
                std::thread::sleep(self.action);
                if let Some(token) = &self.cancel {
                    token.cancel();
                }
                "done"
            }
        };
        ActivityOutcome {
            success: true,
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

/// A probe that passes only when the target prints `ok`.
fn ok_probe(name: &str) -> Activity {
    let mut probe = activity(name, ActivityType::Probe);
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "ok".into(),
    });
    probe
}

/// Runs counted and runs timed, by outcome.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Outcomes {
    counted_success: u64,
    counted_failure: u64,
    timed_success: u64,
    timed_failure: u64,
}

impl Outcomes {
    fn since(self, before: Self) -> Self {
        Self {
            counted_success: self.counted_success - before.counted_success,
            counted_failure: self.counted_failure - before.counted_failure,
            timed_success: self.timed_success - before.timed_success,
            timed_failure: self.timed_failure - before.timed_failure,
        }
    }
}

const ONE_SUCCESS: Outcomes = Outcomes {
    counted_success: 1,
    counted_failure: 0,
    timed_success: 1,
    timed_failure: 0,
};

const ONE_FAILURE: Outcomes = Outcomes {
    counted_success: 0,
    counted_failure: 1,
    timed_success: 0,
    timed_failure: 1,
};

/// The one provider of this binary, and the lock that keeps its tests
/// from recording into each other's readings.
fn meter() -> (
    &'static SdkMeterProvider,
    &'static InMemoryMetricExporter,
    MutexGuard<'static, ()>,
) {
    static METER: OnceLock<(SdkMeterProvider, InMemoryMetricExporter)> = OnceLock::new();
    static ONE_RUN: Mutex<()> = Mutex::new(());
    let (provider, exporter) = METER.get_or_init(|| {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        (provider, exporter)
    });
    let guard = ONE_RUN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (provider, exporter, guard)
}

fn is_success<'a>(mut attributes: impl Iterator<Item = &'a opentelemetry::KeyValue>) -> bool {
    attributes
        .any(|kv| kv.key.as_str() == "resilience.outcome.status" && kv.value.as_str() == "success")
}

/// The cumulative outcome metrics as last exported.
fn outcomes(provider: &SdkMeterProvider, exporter: &InMemoryMetricExporter) -> Outcomes {
    provider.force_flush().expect("flush");
    let mut seen = Outcomes::default();
    for rm in exporter.get_finished_metrics().unwrap_or_default() {
        for metric in rm
            .scope_metrics()
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        {
            match (metric.name(), metric.data()) {
                ("tumult.experiments.total", AggregatedMetrics::U64(MetricData::Sum(sum))) => {
                    (seen.counted_success, seen.counted_failure) = (0, 0);
                    for point in sum.data_points() {
                        if is_success(point.attributes()) {
                            seen.counted_success += point.value();
                        } else {
                            seen.counted_failure += point.value();
                        }
                    }
                }
                (
                    "tumult.experiment.duration",
                    AggregatedMetrics::F64(MetricData::Histogram(histogram)),
                ) => {
                    (seen.timed_success, seen.timed_failure) = (0, 0);
                    for point in histogram.data_points() {
                        if is_success(point.attributes()) {
                            seen.timed_success += point.count();
                        } else {
                            seen.timed_failure += point.count();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    seen
}

/// Run `experiment` against `target` and give its final status with what
/// the run added to the outcome metrics.
fn run(
    experiment: &Experiment,
    target: Scripted,
    config: &RunConfig,
) -> (ExperimentStatus, Outcomes) {
    let (provider, exporter, _one_run) = meter();
    let before = outcomes(provider, exporter);
    let executor: Arc<dyn ActivityExecutor> = Arc::new(target);
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
        recovery_interval: Duration::from_millis(10),
    };
    let journal = run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        config,
        &sampling,
    )
    .expect("run");
    (journal.status, outcomes(provider, exporter).since(before))
}

fn one_action(title: &str) -> Experiment {
    Experiment {
        title: title.into(),
        method: vec![activity("action-1", ActivityType::Action)],
        ..Experiment::default()
    }
}

#[test]
fn a_completed_run_is_counted_and_timed_as_a_success() {
    let target = Scripted {
        probe: "ok",
        action: Duration::ZERO,
        cancel: None,
    };

    let (status, added) = run(&one_action("completed"), target, &RunConfig::default());

    assert_eq!(status, ExperimentStatus::Completed);
    assert_eq!(added, ONE_SUCCESS);
}

#[test]
fn an_aborted_run_is_counted_and_timed_as_a_failure() {
    let experiment = Experiment {
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![ok_probe("steady")],
        }),
        ..one_action("aborted")
    };
    let target = Scripted {
        probe: "down",
        action: Duration::ZERO,
        cancel: None,
    };

    let (status, added) = run(&experiment, target, &RunConfig::default());

    assert_eq!(status, ExperimentStatus::Aborted);
    assert_eq!(added, ONE_FAILURE);
}

#[test]
fn an_interrupted_run_is_counted_and_timed_as_a_failure() {
    let token = CancellationToken::new();
    let config = RunConfig {
        cancellation_token: Some(token.clone()),
        ..RunConfig::default()
    };
    let target = Scripted {
        probe: "ok",
        action: Duration::ZERO,
        cancel: Some(token),
    };

    let (status, added) = run(&one_action("interrupted"), target, &config);

    assert_eq!(status, ExperimentStatus::Interrupted);
    assert_eq!(added, ONE_FAILURE);
}

#[test]
fn a_halted_run_is_counted_in_the_experiment_outcome_metrics() {
    let experiment = Experiment {
        title: "halted".into(),
        method: vec![
            activity("action-1", ActivityType::Action),
            activity("action-2", ActivityType::Action),
            activity("action-3", ActivityType::Action),
        ],
        guards: vec![Guard {
            name: "safety".into(),
            probe: ok_probe("guard-probe"),
            min_breaches: 1,
        }],
        ..Experiment::default()
    };
    // The guard's probe is always unsafe; the actions take long enough
    // for the guard to be seen.
    let target = Scripted {
        probe: "UNSAFE",
        action: Duration::from_millis(400),
        cancel: None,
    };

    let (status, added) = run(&experiment, target, &RunConfig::default());

    assert_eq!(status, ExperimentStatus::Halted);
    assert_eq!(added, ONE_FAILURE);
}

#[test]
fn a_run_whose_load_did_not_start_is_counted_and_timed_as_a_failure() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let tools = tempfile::TempDir::new().expect("tools directory");
    env.set("TUMULT_OHA_BINARY", tools.path().join("no-oha"));
    let experiment = Experiment {
        load: Some(LoadConfig {
            tool: LoadTool::Oha,
            script: "http://127.0.0.1:9/health".into(),
            vus: Some(1),
            duration_s: Some(1.0),
            thresholds: std::collections::HashMap::new(),
            profile: None,
        }),
        ..one_action("load did not start")
    };
    let config = RunConfig {
        load_executor: Some(Arc::new(
            tumult_core::runner::oha::OhaLoadExecutor::default(),
        )),
        ..RunConfig::default()
    };
    let (provider, exporter, _one_run) = meter();
    let before = outcomes(provider, exporter);
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Scripted {
        probe: "ok",
        action: Duration::ZERO,
        cancel: None,
    });

    let run = run_experiment_with_sampling(
        &experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &config,
        &SamplingConfig::default(),
    );

    assert!(
        matches!(&run, Err(tumult_core::runner::RunnerError::LoadStart(_))),
        "{run:?}"
    );
    assert_eq!(outcomes(provider, exporter).since(before), ONE_FAILURE);
}
