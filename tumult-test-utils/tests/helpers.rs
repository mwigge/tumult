//! Integration tests for the shared test helpers themselves.
//!
//! The helpers are consumed by integration suites across the workspace, so
//! their observable behaviour (outcomes, error text, execution order, event
//! recording, builder field shapes) is pinned here against the public API.

use tumult_core::controls::{ControlHandler, LifecycleEvent};
use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{ActivityType, Provider, Tolerance};
use tumult_test_utils::{
    action, background_action, experiment_builder, foreground_action, hypothesis,
    minimal_experiment, probe_with_tolerance, EventLog, MockPlugin,
};

#[test]
fn mock_plugin_default_outcome_succeeds_with_ok_output() {
    let plugin = MockPlugin::new();
    let outcome = plugin.execute(&action("anything"));
    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("200"));
    assert_eq!(outcome.error, None);
}

#[test]
fn mock_plugin_default_impl_matches_new() {
    let outcome = MockPlugin::default().execute(&action("anything"));
    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("200"));
}

#[test]
fn mock_plugin_registered_outcomes_override_default() {
    let plugin = MockPlugin::new()
        .on("health-check", true, Some("alive"))
        .on("kill-switch", false, None);

    let ok = plugin.execute(&action("health-check"));
    assert!(ok.success);
    assert_eq!(ok.output.as_deref(), Some("alive"));
    assert_eq!(ok.error, None);

    let failed = plugin.execute(&action("kill-switch"));
    assert!(!failed.success);
    assert_eq!(failed.output, None);
    assert_eq!(failed.error.as_deref(), Some("kill-switch failed"));
}

#[test]
fn mock_plugin_default_fail_reports_default_failure() {
    let plugin = MockPlugin::new().default_fail();
    let outcome = plugin.execute(&action("unregistered"));
    assert!(!outcome.success);
    assert_eq!(outcome.output, None);
    assert_eq!(outcome.error.as_deref(), Some("default failure"));
}

#[test]
fn mock_plugin_default_output_override_is_used() {
    let plugin = MockPlugin::new().default_output("custom-ok");
    let outcome = plugin.execute(&action("unregistered"));
    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("custom-ok"));
    assert_eq!(outcome.error, None);
}

#[test]
fn mock_plugin_records_execution_order_in_log() {
    let plugin = MockPlugin::new();
    let handle = plugin.execution_log_handle();

    plugin.execute(&action("first"));
    plugin.execute(&action("second"));
    plugin.execute(&action("first"));

    assert_eq!(plugin.log(), vec!["first", "second", "first"]);
    assert_eq!(
        handle.lock().expect("execution log poisoned").as_slice(),
        ["first", "second", "first"]
    );
}

#[test]
fn event_log_records_lifecycle_events_via_handle() {
    let (log, events) = EventLog::new();
    assert_eq!(log.name(), "event-log");
    assert!(events.lock().expect("events poisoned").is_empty());

    log.on_event(&LifecycleEvent::BeforeExperiment);
    log.on_event(&LifecycleEvent::BeforeActivity {
        name: "inject-fault".into(),
    });

    let recorded = events.lock().expect("events poisoned");
    assert_eq!(recorded.len(), 2);
    assert!(matches!(recorded[0], LifecycleEvent::BeforeExperiment));
    assert_eq!(recorded[1].event_name(), "before_activity");
    assert_eq!(recorded[1].activity_name(), Some("inject-fault"));
}

#[test]
fn action_builds_foreground_native_action() {
    let activity = action("inject-fault");
    assert_eq!(activity.name, "inject-fault");
    assert_eq!(activity.activity_type, ActivityType::Action);
    assert!(!activity.background);
    assert_eq!(activity.tolerance, None);
    match &activity.provider {
        Provider::Native {
            plugin, function, ..
        } => {
            assert_eq!(plugin, "mock");
            assert_eq!(function, "noop");
        }
        other => panic!("expected native provider, got: {other:?}"),
    }
}

#[test]
fn background_action_sets_background_flag() {
    let activity = background_action("slow-fault");
    assert!(activity.background);
    assert_eq!(activity.activity_type, ActivityType::Action);
}

#[test]
fn foreground_action_is_a_foreground_alias() {
    let activity = foreground_action("fast-fault");
    assert!(!activity.background);
    assert_eq!(activity.name, "fast-fault");
}

#[test]
fn probe_with_tolerance_builds_process_probe_with_exact_tolerance() {
    let probe = probe_with_tolerance("health", serde_json::json!(200));
    assert_eq!(probe.name, "health");
    assert_eq!(probe.activity_type, ActivityType::Probe);
    assert!(!probe.background);
    assert_eq!(
        probe.tolerance,
        Some(Tolerance::Exact {
            value: serde_json::json!(200)
        })
    );
    match &probe.provider {
        Provider::Process {
            path, timeout_s, ..
        } => {
            assert_eq!(path, "scripts/health-check.sh");
            assert_eq!(*timeout_s, Some(5.0));
        }
        other => panic!("expected process provider, got: {other:?}"),
    }
}

#[test]
fn hypothesis_carries_title_and_probes() {
    let probes = vec![
        probe_with_tolerance("p1", serde_json::json!(200)),
        probe_with_tolerance("p2", serde_json::json!("ok")),
    ];
    let hyp = hypothesis("the system stays up", probes);
    assert_eq!(hyp.title, "the system stays up");
    assert_eq!(hyp.probes.len(), 2);
    assert_eq!(hyp.probes[0].name, "p1");
    assert_eq!(hyp.probes[1].name, "p2");
}

#[test]
fn experiment_builder_provides_lifecycle_skeleton() {
    let experiment = experiment_builder();
    assert_eq!(experiment.version, "v1");
    assert_eq!(experiment.title, "Integration test experiment");
    assert_eq!(
        experiment.description.as_deref(),
        Some("Tests the full five-phase lifecycle")
    );
    assert_eq!(experiment.tags, vec!["integration", "test"]);
    assert_eq!(experiment.method.len(), 1);
    assert_eq!(experiment.method[0].name, "inject-fault");
    assert_eq!(experiment.method[0].activity_type, ActivityType::Action);
    assert!(experiment.steady_state_hypothesis.is_none());
    assert!(experiment.rollbacks.is_empty());
}

#[test]
fn minimal_experiment_uses_supplied_method() {
    let method = vec![action("a"), background_action("b")];
    let experiment = minimal_experiment(method);
    assert_eq!(experiment.title, "minimal test experiment");
    assert_eq!(experiment.description, None);
    assert!(experiment.tags.is_empty());
    assert_eq!(experiment.method.len(), 2);
    assert_eq!(experiment.method[0].name, "a");
    assert!(!experiment.method[0].background);
    assert_eq!(experiment.method[1].name, "b");
    assert!(experiment.method[1].background);
    assert!(experiment.steady_state_hypothesis.is_none());
    assert!(experiment.rollbacks.is_empty());
}
