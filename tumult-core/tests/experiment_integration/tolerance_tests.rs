//! Additional integration coverage: regex tolerance and multi-probe hypotheses.

use std::collections::HashMap;

use crate::common::*;

// ═══════════════════════════════════════════════════════════════
// Additional integration: regex tolerance
// ═══════════════════════════════════════════════════════════════

#[test]
fn regex_tolerance_in_hypothesis() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Status matches pattern",
        vec![Activity {
            name: "status-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/status".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Regex {
                pattern: "^OK.*".into(),
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
    ));

    let plugin: Arc<dyn ActivityExecutor> = Arc::new(
        MockPlugin::new()
            .on("status-probe", true, Some("\"OK: all systems go\""))
            .default_output("200"),
    );
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
}

// ═══════════════════════════════════════════════════════════════
// Additional integration: multiple hypothesis probes
// ═══════════════════════════════════════════════════════════════

#[test]
fn multiple_hypothesis_probes_all_must_pass() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "All services healthy",
        vec![
            probe_with_tolerance("api-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("db-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("cache-health", serde_json::Value::Number(200.into())),
        ],
    ));

    // All probes pass
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(MockPlugin::new().default_output("200"));
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();
    assert_eq!(journal.status, ExperimentStatus::Completed);
}

#[test]
fn one_failing_hypothesis_probe_causes_abort() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "All services healthy",
        vec![
            probe_with_tolerance("api-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("db-health", serde_json::Value::Number(200.into())),
        ],
    ));

    // db-health returns 503 → one probe fails → hypothesis fails
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(
        MockPlugin::new()
            .on("api-health", true, Some("200"))
            .on("db-health", true, Some("503")),
    );
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();
    assert_eq!(journal.status, ExperimentStatus::Aborted);
}
