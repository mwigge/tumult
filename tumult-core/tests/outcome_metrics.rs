//! The experiment outcome metrics (`tumult.experiments.total`,
//! `tumult.experiment.duration`) are recorded for a run that a guard
//! halted, as for any other run.
//!
//! One test only: the metrics bind to the meter provider that is global
//! when the runner first records, so this binary installs its own first.

use std::sync::Arc;
use std::time::Duration;

use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;

/// The guard's probe is always unsafe; method actions take long enough
/// for the guard to be seen.
struct Unsafe;

impl ActivityExecutor for Unsafe {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let output = match activity.activity_type {
            ActivityType::Probe => "UNSAFE",
            ActivityType::Action => {
                std::thread::sleep(Duration::from_millis(400));
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

fn metric_names(provider: &SdkMeterProvider, exporter: &InMemoryMetricExporter) -> Vec<String> {
    provider.force_flush().expect("flush");
    exporter
        .get_finished_metrics()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|rm| {
            rm.scope_metrics()
                .flat_map(|sm| {
                    sm.metrics()
                        .map(|m| m.name().to_string())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_halted_run_is_counted_in_the_experiment_outcome_metrics() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    opentelemetry::global::set_meter_provider(provider.clone());

    let mut probe = activity("guard-probe", ActivityType::Probe);
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "ok".into(),
    });
    let experiment = Experiment {
        title: "halted".into(),
        method: vec![
            activity("action-1", ActivityType::Action),
            activity("action-2", ActivityType::Action),
            activity("action-3", ActivityType::Action),
        ],
        guards: vec![Guard {
            name: "safety".into(),
            probe,
            min_breaches: 1,
        }],
        ..Experiment::default()
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Unsafe);
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
    };

    let journal = run_experiment_with_sampling(
        &experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("run");
    assert_eq!(journal.status, ExperimentStatus::Halted);

    let names = metric_names(&provider, &exporter);
    assert!(
        names.iter().any(|n| n == "tumult.experiments.total"),
        "a halted run must be counted: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "tumult.experiment.duration"),
        "a halted run must be timed: {names:?}"
    );
    let _ = provider.shutdown();
}
